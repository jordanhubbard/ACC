self.addEventListener('install', (event) => {
  event.waitUntil(caches.open('squirrelchat-v1').then(cache => cache.add('/')));
});
self.addEventListener('fetch', (event) => {
  event.respondWith(
    caches.match(event.request).then(resp => resp || fetch(event.request))
  );
});
