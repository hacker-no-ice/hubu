/** A small first-party tracker. Failure must never interrupt the public site. */
export const BEACON_JS = String.raw`(function () {
  'use strict';
  try {
    if (window.__hubuAnalyticsLoaded || window.top !== window.self || !/^https?:$/.test(location.protocol)) return;
    window.__hubuAnalyticsLoaded = true;
    var lastPath = null;
    var memorySession = null;
    var queue = Promise.resolve();
    var sessionKey = 'hubu_analytics_session';
    var idleMs = 30 * 60 * 1000;
    var uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

    function optedOut() {
      return navigator.globalPrivacyControl === true || navigator.doNotTrack === '1' ||
        navigator.msDoNotTrack === '1' || window.doNotTrack === '1' ||
        document.cookie.split(';').some(function (cookie) { return cookie.trim() === 'hubu_analytics_exclude=1'; });
    }

    function sessionId() {
      var now = Date.now();
      var session = memorySession;
      try {
        var stored = sessionStorage.getItem(sessionKey);
        if (stored) session = JSON.parse(stored);
      } catch (_) { /* Storage can be unavailable in privacy modes. */ }
      if (!session || !uuidPattern.test(session.id) || !Number.isFinite(session.lastAt) ||
          session.lastAt > now || now - session.lastAt >= idleMs) {
        session = { id: crypto.randomUUID(), lastAt: now };
      }
      session.lastAt = now;
      memorySession = session;
      try { sessionStorage.setItem(sessionKey, JSON.stringify(session)); } catch (_) { /* Use memory for this page. */ }
      return session.id;
    }

    // Keep the complete JSON comfortably below 2 KB, including Unicode paths.
    function bounded(value) {
      var result = '';
      var bytes = 0;
      var encoder = new TextEncoder();
      for (var character of String(value || '')) {
        var size = encoder.encode(character).length;
        if (result.length + character.length > 512 || bytes + size > 700) break;
        result += character;
        bytes += size;
      }
      return result;
    }

    function referrerOrigin() {
      try {
        var referrer = new URL(document.referrer);
        return /^https?:$/.test(referrer.protocol) ? bounded(referrer.origin) : '';
      } catch (_) { return ''; }
    }

    function track(force) {
      try {
        var path = location.pathname;
        if (document.prerendering || document.visibilityState !== 'visible' ||
            /^\/_analytics(?:\/|$)/.test(path) || optedOut() || (!force && path === lastPath)) return;
        var payload = JSON.stringify({
          eventId: crypto.randomUUID(),
          sessionId: sessionId(),
          path: bounded(path),
          referrer: referrerOrigin(),
          automated: navigator.webdriver === true
        });
        lastPath = path;
        // Serialize sends so the first response sets the visitor cookie before
        // another client navigation can send a request without that identity.
        queue = queue.then(function () {
          if (optedOut()) return;
          return fetch('/_telemetry/collect', {
            method: 'POST', credentials: 'same-origin', keepalive: true,
            headers: { 'content-type': 'application/json' }, body: payload
          });
        }).catch(function () { /* Analytics is best effort. */ });
      } catch (_) { /* No analytics failure should affect navigation. */ }
    }

    ['pushState', 'replaceState'].forEach(function (method) {
      var original = history[method];
      history[method] = function () {
        var result = original.apply(this, arguments);
        track(false);
        return result;
      };
    });
    window.addEventListener('popstate', function () { track(false); });
    window.addEventListener('pageshow', function (event) { track(event.persisted === true); });
    document.addEventListener('visibilitychange', function () { track(false); });
    document.addEventListener('prerenderingchange', function () { track(false); });
    track(false);
  } catch (_) { /* A blocked browser API must not affect the site. */ }
})();`;
