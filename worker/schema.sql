-- Safe to run again (npm run schema:remote). Columns added after a table first shipped are in migrate.sql.
CREATE TABLE IF NOT EXISTS events(
    id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, install_id TEXT NOT NULL, version TEXT NOT NULL,
    os TEXT NOT NULL, client TEXT NOT NULL, kind TEXT NOT NULL, files INTEGER NOT NULL, bytes INTEGER NOT NULL,
    duplicate_bytes INTEGER NOT NULL, junk_bytes INTEGER NOT NULL, old_download_bytes INTEGER NOT NULL, large_bytes INTEGER NOT NULL,
    -- country: Cloudflare's guess from the IP (the IP itself is never stored); plan free|pro; channel web|appstore|playstore;
    -- detail: what a "vault"/"pro"/"bought" event was about. NULL on events from apps older than 0.4.
    country TEXT, plan TEXT, channel TEXT, detail TEXT);
CREATE INDEX IF NOT EXISTS events_install ON events(install_id, ts);
CREATE INDEX IF NOT EXISTS events_ts ON events(ts);
CREATE TABLE IF NOT EXISTS hardware(install_id TEXT PRIMARY KEY, ts INTEGER NOT NULL, specs TEXT NOT NULL);
-- Pro subscriptions from Paddle webhooks (email asked from Paddle's API), plus licenses issued by hand in /admin (customer_id "manual").
-- A license key works on up to 3 installations (activations).
CREATE TABLE IF NOT EXISTS subscriptions(
    id TEXT PRIMARY KEY, customer_id TEXT NOT NULL, status TEXT NOT NULL, period_end INTEGER NOT NULL,
    license_key TEXT NOT NULL UNIQUE, install_id TEXT, txn TEXT, event_at TEXT NOT NULL,
    -- ref: the order reference from the /buy page (custom_data), so /welcome can show the key of this purchase.
    plan TEXT, created_at INTEGER, email TEXT, ref TEXT);
CREATE INDEX IF NOT EXISTS subscriptions_install ON subscriptions(install_id);
CREATE INDEX IF NOT EXISTS subscriptions_txn ON subscriptions(txn);
CREATE INDEX IF NOT EXISTS subscriptions_ref ON subscriptions(ref);
-- plx (src/plx.ts): Pro paid in USDC on Solana. reference: the Solana Pay reference key the payment carries;
-- amount in USDC base units (6 decimals); signature: the paying transaction, once found (one transaction pays one invoice).
CREATE TABLE IF NOT EXISTS plx_invoices(ref TEXT PRIMARY KEY, reference TEXT NOT NULL, plan TEXT NOT NULL, amount INTEGER NOT NULL,
    install_id TEXT, email TEXT, license_key TEXT, created_at INTEGER NOT NULL, checked_at INTEGER NOT NULL DEFAULT 0,
    signature TEXT UNIQUE, paid_at INTEGER);
CREATE INDEX IF NOT EXISTS plx_invoices_open ON plx_invoices(created_at) WHERE signature IS NULL;
CREATE TABLE IF NOT EXISTS activations(license_key TEXT NOT NULL, install_id TEXT NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY(license_key, install_id));
-- Installations the operator marked as their own: listed, but left out of every count.
CREATE TABLE IF NOT EXISTS ignored_installs(install_id TEXT PRIMARY KEY, ts INTEGER NOT NULL);
-- What was done in /admin, newest first in the panel.
CREATE TABLE IF NOT EXISTS admin_log(id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, action TEXT NOT NULL, ref TEXT, detail TEXT NOT NULL);
-- Messages sent from the app's contact form (also mailed to OWNER_EMAIL when mail is set up).
CREATE TABLE IF NOT EXISTS messages(id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, install_id TEXT NOT NULL, email TEXT, message TEXT NOT NULL,
    version TEXT NOT NULL, os TEXT NOT NULL, country TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS messages_ts ON messages(ts);
-- Website visits and download clicks, with where they came from (src/site.ts sourceOf); no IP address, no cookie.
CREATE TABLE IF NOT EXISTS visits(id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, kind TEXT NOT NULL, source TEXT NOT NULL, country TEXT, platform TEXT);
CREATE INDEX IF NOT EXISTS visits_ts ON visits(ts);
