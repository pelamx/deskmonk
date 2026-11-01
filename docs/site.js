// deskmonk.app: the page arrives already in the visitor's language (worker/src/site.ts). Here: the language menu,
// the monthly/yearly price switch, and the download for the visitor's computer.

// Language menu: the choice is kept in the "lang" cookie, which the worker and the /buy pages read.
const menu = document.querySelector("select.langs");
if (menu) {
  menu.addEventListener("change", () => {
    document.cookie = `lang=${menu.value}; Path=/; Max-Age=31536000; SameSite=Lax; Secure`;
    const u = new URL(location.href);
    u.searchParams.set("lang", menu.value);
    location.href = u.pathname + u.search + u.hash;
  });
}

// Monthly / yearly switch on the pricing section; the words and amounts come in the page's language.
const price = document.getElementById("pro-price");
if (price) {
  const words = document.getElementById("cycle-words");
  const text = {
    monthly: { amount: price.firstElementChild.textContent, per: price.querySelector("small").textContent, note: document.getElementById("pro-sub").innerHTML },
    yearly: { amount: words.children[0].textContent, per: words.children[1].textContent, note: words.children[2].innerHTML },
  };
  const buy = document.getElementById("pro-buy");
  const lang = new URL(buy.href).searchParams.get("lang");
  const show = (cycle) => {
    for (const b of document.querySelectorAll(".cycle button")) b.classList.toggle("on", b.dataset.cycle === cycle);
    price.firstElementChild.textContent = text[cycle].amount;
    price.querySelector("small").textContent = text[cycle].per;
    document.getElementById("pro-sub").innerHTML = text[cycle].note;
    buy.setAttribute("href", `/buy?plan=${cycle}${lang ? `&lang=${lang}` : ""}`);
  };
  for (const b of document.querySelectorAll(".cycle button")) b.addEventListener("click", () => show(b.dataset.cycle));
}

// Download and Get Pro clicks, for Google Tag Manager (worker/src/site.ts). Where it isn't loaded, this list stays in the page.
document.addEventListener("click", (e) => {
  const a = e.target.closest("a");
  if (!a) return;
  const download = a.pathname.startsWith("/downloads/") || a.hostname === "github.com";
  if (download || a.pathname === "/buy") (window.dataLayer = window.dataLayer || []).push({ event: download ? "download_click" : "get_pro_click" });
});

// The download that fits the visitor's computer.
const ua = navigator.userAgent;
const os = /Mac/.test(ua) && !/iPhone|iPad/.test(ua) ? "mac" : /Windows/.test(ua) ? "win" : /Linux|X11/.test(ua) && !/Android/.test(ua) ? "linux" : null;
const card = os && document.querySelector(`.dl[data-os="${os}"]`);
if (card) {
  card.classList.add("mine");
  const heroDl = document.getElementById("hero-dl");
  const link = card.querySelector("a.btn");
  if (heroDl && link) {
    heroDl.href = link.href;
    document.getElementById("hero-os").textContent = card.querySelector("h3").textContent + " · " + link.querySelector("small").textContent;
  }
}
