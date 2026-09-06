// The service worker: the shell's files offline, and notifications from the
// runtime shown even when no tab is open. Nothing here talks to the wire.
const SHELL = ["/", "/style.css", "/app.js", "/wire.js", "/frames.js", "/render.js", "/icon.svg", "/manifest.webmanifest"];
const CACHE = "ultraplexr-shell-v1";

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(caches.keys().then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k)))).then(() => self.clients.claim()));
});

// Network first for the shell, so a republished runtime wins; cache when offline.
self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== "GET" || url.origin !== self.location.origin || url.pathname === "/ws") return;
  event.respondWith(
    fetch(event.request)
      .then((response) => {
        if (response.ok) caches.open(CACHE).then((cache) => cache.put(event.request, response.clone()));
        return response;
      })
      .catch(() => caches.match(event.request).then((hit) => hit ?? new Response("offline", { status: 503 }))),
  );
});

self.addEventListener("push", (event) => {
  let notice = { title: "ultraplexr", body: "", url: "/", tag: "" };
  try { notice = { ...notice, ...event.data.json() }; } catch {}
  event.waitUntil(self.registration.showNotification(notice.title, {
    body: notice.body,
    tag: notice.tag || undefined,
    icon: "/icon.svg",
    badge: "/icon.svg",
    data: { url: notice.url },
  }));
});

// A tap lands on the exact thing: an open tab is focused and steered, else one is opened.
self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const target = new URL(event.notification.data?.url ?? "/", self.location.origin).href;
  event.waitUntil(self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((all) => {
    const tab = all.find((c) => "focus" in c);
    if (tab) { tab.navigate ? tab.navigate(target).then((c) => c?.focus()) : tab.focus(); return; }
    return self.clients.openWindow(target);
  }));
});
