//! Vault: a file or folder is packed with tar and encrypted into one `.dmvault` file.
//! The key comes from the password (Argon2id, fresh salt per vault); the data is sealed in 64 KiB
//! ChaCha20-Poly1305 chunks (the STREAM construction), so a changed, reordered or cut chunk is caught.
//! Layout: magic | salt (16) | nonce (7) | sealed chunks; the header is authenticated with every chunk.

use crate::i18n::t;
use crate::tf;
use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::aead::{KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const EXT: &str = "dmvault";
const MAGIC: &[u8; 8] = b"DMVAULT1";
const HEADER: usize = 8 + 16 + 7;
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;

fn list_path() -> Option<PathBuf> {
    Some(crate::config_dir()?.join("vaults"))
}

/// Vaults made with deskMonk, newest first, so they can be found and opened again.
// shortcut: one path per line, so a file name containing a line break is not remembered correctly.
pub fn list() -> Vec<PathBuf> {
    let Some(text) = list_path().and_then(|p| fs::read_to_string(p).ok()) else {
        // No list yet: vaults made before the list existed are looked up once.
        let found = find_vaults();
        let _ = save_list(&found);
        return found;
    };
    text.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect()
}

/// .dmvault files in the home folder, through Spotlight. The sandboxed build may not run it and finds none.
fn find_vaults() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir().filter(|_| cfg!(target_os = "macos") && !crate::sandboxed()) else {
        return Vec::new();
    };
    let query = format!("kMDItemFSName == \"*.{EXT}\"");
    let out = std::process::Command::new("mdfind").arg("-onlyin").arg(&home).arg(query).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    text.lines().map(PathBuf::from).filter(|p| is_vault(p) && !p.starts_with(home.join(".Trash"))).collect()
}

pub fn save_list(vaults: &[PathBuf]) -> io::Result<()> {
    let path = list_path().ok_or_else(|| io::Error::other("no config folder"))?;
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, vaults.iter().map(|v| format!("{}\n", v.display())).collect::<String>())
}

pub fn is_vault(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == EXT)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = OsString::from(path);
    s.push(suffix);
    s.into()
}

/// Bytes the progress counter reaches for `path`: its file sizes to encrypt, the vault size to decrypt.
pub fn work_size(path: &Path) -> u64 {
    walkdir::WalkDir::new(path).into_iter().flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()
}

fn cipher(password: &str, salt: &[u8]) -> Result<ChaCha20Poly1305, String> {
    let mut key = [0u8; 32];
    argon2::Argon2::default().hash_password_into(password.as_bytes(), salt, &mut key).map_err(|e| e.to_string())?;
    Ok(ChaCha20Poly1305::new(Key::from_slice(&key)))
}

/// Encrypts `src` into `<src>.dmvault` next to it and returns that path, once the vault has been read back and
/// opens with `password`. `src` itself is left in place; see [`destroy`].
pub fn encrypt(src: &Path, password: &str, done: &AtomicU64) -> Result<PathBuf, String> {
    let name = src.file_name().ok_or_else(|| t("Geçersiz yol", "Invalid path", "Ruta no válida").to_string())?;
    let out = with_suffix(src, &format!(".{EXT}"));
    if out.exists() {
        return Err(tf!("{} zaten var", "{} already exists", "{} ya existe", out.display()));
    }
    let mut header = [0u8; HEADER];
    header[..8].copy_from_slice(MAGIC);
    getrandom::fill(&mut header[8..]).map_err(|e| e.to_string())?;
    let cipher = cipher(password, &header[8..24])?;
    let part = with_suffix(&out, ".part");
    let result = (|| -> io::Result<()> {
        let mut file = BufWriter::new(File::create(&part)?);
        file.write_all(&header)?;
        let mut sealer = Sealer {
            enc: EncryptorBE32::from_aead(cipher, header[24..].into()),
            buf: Vec::with_capacity(CHUNK),
            out: file,
            aad: header,
            done,
        };
        let mut tar = tar::Builder::new(&mut sealer);
        tar.follow_symlinks(false);
        if src.is_dir() {
            tar.append_dir_all(name, src)?;
        } else {
            tar.append_path_with_name(src, name)?;
        }
        tar.into_inner()?;
        sealer.finish()?;
        // Read back before anyone deletes the original: every chunk must open with this password.
        io::copy(&mut open(&part, password, done).map_err(io::Error::other)?, &mut io::sink())?;
        Ok(())
    })();
    match result.and_then(|()| fs::rename(&part, &out)) {
        Ok(()) => Ok(out),
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e.to_string())
        }
    }
}

/// The decrypted content of a vault, as a tar stream; checks the password on the first chunk.
fn open<'a>(vault: &Path, password: &str, done: &'a AtomicU64) -> Result<Opener<'a, BufReader<File>>, String> {
    let mut file = BufReader::new(File::open(vault).map_err(|e| e.to_string())?);
    let mut header = [0u8; HEADER];
    if file.read_exact(&mut header).is_err() || &header[..8] != MAGIC {
        return Err(t("Bu bir deskMonk kasası değil", "This is not a deskMonk vault", "No es una caja fuerte de deskMonk").into());
    }
    let mut opener = Opener {
        dec: Some(DecryptorBE32::from_aead(cipher(password, &header[8..24])?, header[24..].into())),
        input: file,
        aad: header,
        plain: Vec::new(),
        pos: 0,
        done,
    };
    // The first chunk tells a wrong password apart from a damaged file.
    if opener.next_chunk().is_err() {
        return Err(t("Yanlış şifre", "Wrong password", "Contraseña incorrecta").into());
    }
    Ok(opener)
}

/// Opens a vault next to itself and returns the restored file or folder. The vault is left in place.
pub fn decrypt(vault: &Path, password: &str, done: &AtomicU64) -> Result<PathBuf, String> {
    let parent = vault.parent().ok_or_else(|| t("Geçersiz yol", "Invalid path", "Ruta no válida").to_string())?;
    let mut opener = open(vault, password, done)?;
    // Unpacked into a hidden folder first, so a damaged vault leaves nothing half-restored.
    let tmp = parent.join(format!(".deskmonk-open-{}", uuid::Uuid::new_v4()));
    let result = (|| -> io::Result<PathBuf> {
        fs::create_dir(&tmp)?;
        tar::Archive::new(&mut opener).unpack(&tmp)?;
        let mut entries = fs::read_dir(&tmp)?.collect::<io::Result<Vec<_>>>()?;
        let (Some(entry), None) = (entries.pop(), entries.pop()) else {
            return Err(io::Error::other(t("Kasa boş ya da bozuk", "The vault is empty or damaged", "La caja fuerte está vacía o dañada")));
        };
        let dest = parent.join(entry.file_name());
        if dest.symlink_metadata().is_ok() {
            return Err(io::Error::other(tf!("{} zaten var", "{} already exists", "{} ya existe", dest.display())));
        }
        fs::rename(entry.path(), &dest)?;
        Ok(dest)
    })();
    let _ = fs::remove_dir_all(&tmp);
    result.map_err(|e| e.to_string())
}

/// Deletes the unencrypted original for good after encrypting: each file is overwritten with zeros, then removed.
/// Not the Trash, which would keep a readable copy. Symlinks are removed, not followed.
// shortcut: on SSDs and copy-on-write file systems (Btrfs, APFS) old blocks may survive the overwrite; full-disk encryption covers that.
pub fn destroy(src: &Path) -> io::Result<()> {
    for entry in walkdir::WalkDir::new(src).into_iter().flatten().filter(|e| e.file_type().is_file()) {
        if let Ok(mut file) = fs::OpenOptions::new().write(true).open(entry.path()) {
            let len = entry.metadata().map_or(0, |m| m.len());
            let zeros = vec![0u8; CHUNK];
            let mut left = len;
            while left > 0 {
                let n = left.min(CHUNK as u64) as usize;
                if file.write_all(&zeros[..n]).is_err() {
                    break;
                }
                left -= n as u64;
            }
            let _ = file.sync_all();
        }
    }
    if src.symlink_metadata()?.is_dir() { fs::remove_dir_all(src) } else { fs::remove_file(src) }
}

/// Encrypts what is written to it, a chunk at a time.
struct Sealer<'a, W: Write> {
    enc: EncryptorBE32<ChaCha20Poly1305>,
    buf: Vec<u8>,
    out: W,
    aad: [u8; HEADER],
    done: &'a AtomicU64,
}

impl<W: Write> Write for Sealer<'_, W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let n = data.len().min(CHUNK - self.buf.len());
        self.buf.extend_from_slice(&data[..n]);
        // A full chunk is never the last one: `finish` always seals a shorter (maybe empty) final chunk.
        if self.buf.len() == CHUNK {
            let sealed = self.enc.encrypt_next(Payload { msg: &self.buf, aad: &self.aad }).map_err(|_| io::Error::other("encrypt"))?;
            self.out.write_all(&sealed)?;
            self.buf.clear();
        }
        self.done.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<W: Write> Sealer<'_, W> {
    fn finish(mut self) -> io::Result<()> {
        let sealed = self.enc.encrypt_last(Payload { msg: &self.buf, aad: &self.aad }).map_err(|_| io::Error::other("encrypt"))?;
        self.out.write_all(&sealed)?;
        self.out.flush()
    }
}

/// Decrypts a vault's chunks as they are read.
struct Opener<'a, R: Read> {
    /// None after the final chunk.
    dec: Option<DecryptorBE32<ChaCha20Poly1305>>,
    input: R,
    aad: [u8; HEADER],
    plain: Vec<u8>,
    pos: usize,
    done: &'a AtomicU64,
}

impl<R: Read> Opener<'_, R> {
    fn next_chunk(&mut self) -> io::Result<()> {
        let mut sealed = Vec::with_capacity(CHUNK + TAG);
        (&mut self.input).take((CHUNK + TAG) as u64).read_to_end(&mut sealed)?;
        self.done.fetch_add(sealed.len() as u64, Ordering::Relaxed);
        let payload = Payload { msg: &sealed, aad: &self.aad };
        let plain = if sealed.len() == CHUNK + TAG {
            self.dec.as_mut().map(|d| d.decrypt_next(payload))
        } else {
            self.dec.take().map(|d| d.decrypt_last(payload))
        };
        let damaged = || io::Error::new(io::ErrorKind::InvalidData, t("Kasa dosyası bozuk", "The vault file is damaged", "El archivo de la caja fuerte está dañado"));
        self.plain = plain.ok_or_else(damaged)?.map_err(|_| damaged())?;
        self.pos = 0;
        Ok(())
    }
}

impl<R: Read> Read for Opener<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.plain.len() {
            if self.dec.is_none() {
                return Ok(0);
            }
            self.next_chunk()?;
        }
        let n = out.len().min(self.plain.len() - self.pos);
        out[..n].copy_from_slice(&self.plain[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_rejects_wrong_password_or_tampering() {
        let dir = std::env::temp_dir().join(format!("deskmonk-vault-{}", std::process::id()));
        let src = dir.join("Secret");
        fs::create_dir_all(src.join("sub")).unwrap();
        let big: Vec<u8> = (0..CHUNK * 2 + 123).map(|i| (i * 7) as u8).collect();
        fs::write(src.join("sub/big.bin"), &big).unwrap();
        fs::write(src.join("note.txt"), "hello").unwrap();
        let done = AtomicU64::new(0);

        let vault = encrypt(&src, "pw1234", &done).unwrap();
        assert!(is_vault(&vault) && encrypt(&src, "pw1234", &done).is_err(), "no overwrite");
        fs::rename(&src, dir.join("orig")).unwrap();
        assert_eq!(decrypt(&vault, "nope", &done).unwrap_err(), t("Yanlış şifre", "Wrong password", "Contraseña incorrecta"));
        let back = decrypt(&vault, "pw1234", &done).unwrap();
        assert_eq!(back, src);
        assert_eq!(fs::read(src.join("sub/big.bin")).unwrap(), big);
        assert_eq!(fs::read_to_string(src.join("note.txt")).unwrap(), "hello");
        assert!(decrypt(&vault, "pw1234", &done).is_err(), "no overwrite");
        fs::remove_dir_all(&src).unwrap();

        // One flipped byte in the second chunk, or a cut-off end: refused, and nothing is restored.
        let good = fs::read(&vault).unwrap();
        let mut bad = good.clone();
        bad[HEADER + CHUNK + TAG + 5] ^= 1;
        fs::write(&vault, &bad).unwrap();
        assert!(decrypt(&vault, "pw1234", &done).is_err());
        fs::write(&vault, &good[..HEADER + CHUNK + TAG]).unwrap();
        assert!(decrypt(&vault, "pw1234", &done).is_err());
        assert!(!src.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2, "no temp folders left: {:?}", fs::read_dir(&dir).unwrap().collect::<Vec<_>>());

        // A single file works the same way; destroy leaves nothing of it.
        let file = dir.join("orig/note.txt");
        let vault = encrypt(&file, "x", &done).unwrap();
        destroy(&file).unwrap();
        assert!(!file.exists());
        assert_eq!(decrypt(&vault, "x", &done).unwrap(), file);
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
        destroy(&dir.join("orig")).unwrap();
        assert!(!dir.join("orig").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
