/** Served only after server-side authentication; contains no credentials. */
export const DASHBOARD_HTML = String.raw`<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="robots" content="noindex,nofollow">
  <meta name="color-scheme" content="dark">
  <title>Visitor analytics · Hubu</title>
  <link rel="icon" href="/favicon.svg" type="image/svg+xml">
  <style>
    :root{--bg:#050505;--surface:#0d0e10;--surface-2:#15161a;--line:#292b31;--text:#f2f2ef;--muted:#a2a4aa;--dim:#73757c;--indigo:#9697ff;--cyan:#71d7e8;--warning:#e8b86b}
    *{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:14px/1.5 ui-sans-serif,system-ui,sans-serif}a{color:inherit;text-decoration:none}button{font:inherit;color:inherit;cursor:pointer}button:focus-visible,a:focus-visible,summary:focus-visible{outline:2px solid var(--cyan);outline-offset:4px}button:disabled{cursor:wait;opacity:.55}.sr-only{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}[hidden]{display:none!important}
    .topbar{height:70px;border-bottom:1px solid #1a1b1f;display:flex;align-items:center;justify-content:space-between;padding:0 max(24px,calc((100vw - 1232px)/2));gap:20px}.brand{display:flex;align-items:center;gap:18px}.brand img{width:80px;height:auto}.brand span{border-left:1px solid var(--line);padding-left:18px;color:var(--muted);font-size:13px}.site-link{color:var(--muted);font-size:13px}.site-link:hover{color:white}main{max-width:1280px;margin:0 auto;padding:50px 24px 48px}.eyebrow{color:var(--cyan);font:11px ui-monospace,monospace;letter-spacing:.12em;text-transform:uppercase;margin:0 0 12px}.heading{display:flex;justify-content:space-between;align-items:flex-end;gap:28px;margin-bottom:26px}h1{font-size:clamp(32px,4vw,45px);font-weight:560;line-height:1.08;letter-spacing:-.055em;margin:0 0 13px}.lede{margin:0;color:var(--muted);max-width:650px}.badge{display:inline-flex;align-items:center;gap:7px;color:#b8c8c5;font-size:11px;padding:5px 9px;border:1px solid #2d3b39;border-radius:20px;white-space:nowrap}.badge:before{content:"";height:5px;width:5px;background:var(--cyan);border-radius:50%}.controls{display:flex;align-items:center;gap:10px;flex-wrap:wrap}.range{display:flex;gap:3px;background:var(--surface);border:1px solid var(--line);padding:4px;border-radius:9px}.range button{border:0;background:none;color:var(--muted);border-radius:5px;padding:7px 12px;font-size:12px;white-space:nowrap}.range button[aria-pressed=true]{background:#272832;color:var(--text)}.button{background:var(--surface);border:1px solid var(--line);border-radius:8px;padding:10px 13px;font-size:12px;white-space:nowrap}.button:hover{border-color:#51535d}.status-line{display:flex;align-items:center;justify-content:space-between;gap:16px;min-height:30px;color:var(--dim);font-size:12px;margin-bottom:14px}.status-line strong{font-weight:500;color:var(--muted)}.notice{border:1px solid #5d4930;border-radius:9px;background:#17120c;color:#e8cda6;padding:13px 16px;margin:0 0 20px}.empty{border:1px dashed #3c3f47;border-radius:10px;padding:22px;color:var(--muted);margin:0 0 20px}.empty strong{display:block;color:var(--text);font-weight:500;margin-bottom:5px}.metrics{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:14px;margin-bottom:20px}.metric{background:var(--surface);border:1px solid var(--line);padding:22px 24px;border-radius:12px;min-width:0}.metric.primary{background:radial-gradient(ellipse at 0 0,rgba(150,151,255,.1),transparent 75%),var(--surface);border-color:#414057}.metric h2{font-size:12px;font-weight:500;color:var(--muted);margin:0 0 12px}.metric .number{font-size:clamp(32px,4vw,43px);font-weight:520;letter-spacing:-.055em;line-height:1.05;font-variant-numeric:tabular-nums}.metric.primary .number{color:#c3c4ff}.metric p{color:var(--dim);font-size:11px;line-height:1.5;margin:10px 0 0}.panel{background:var(--surface);border:1px solid var(--line);border-radius:12px;min-width:0;overflow:hidden}.panel-head{display:flex;align-items:baseline;justify-content:space-between;gap:16px;padding:20px 22px 0}.panel h2{font-size:15px;font-weight:530;letter-spacing:-.015em;margin:0}.panel-head p{margin:0;color:var(--dim);font-size:11px}.trend{padding:0 20px 12px;margin-bottom:20px}.trend .panel-head{padding:20px 2px 0}.chart{height:230px;position:relative;margin-top:12px}.chart svg{height:100%;width:100%;overflow:visible}.chart text{font-family:ui-monospace,monospace;font-size:11px;fill:var(--dim)}.chart .grid{stroke:#25262c;stroke-dasharray:3 5}.chart .bar{fill:var(--indigo)}.chart .bar:hover{fill:var(--cyan)}.chart .baseline{stroke:#383a43}.legend{display:flex;align-items:center;gap:7px;font-size:11px;color:var(--muted);margin:3px 2px 12px}.legend:before{content:"";width:7px;height:7px;border-radius:2px;background:var(--indigo)}details{border-top:1px solid #222329;padding:12px 2px 0}summary{width:fit-content;color:var(--muted);font-size:12px;cursor:pointer}.table-wrap{overflow:auto}table{width:100%;border-collapse:collapse;text-align:left;font-size:12px}th{font-weight:450;color:var(--dim);font-size:10px;text-transform:uppercase;letter-spacing:.04em;white-space:nowrap}td,th{padding:13px 22px;border-bottom:1px solid #202126}td{color:#c7c8cc}td:first-child{color:#e0e0df;overflow-wrap:anywhere;min-width:120px}td.num,th.num{text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap}tbody tr:last-child td{border-bottom:0}.panels{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:20px}.panel .table-wrap{margin-top:13px}.table-empty{padding:26px 22px;color:var(--dim);font-size:12px}.privacy{margin-top:28px;border-top:1px solid var(--line);padding-top:24px;display:grid;grid-template-columns:minmax(0,1fr) minmax(230px,.48fr);gap:48px}.privacy h2{font-size:13px;font-weight:550;margin:0 0 9px}.privacy p{font-size:12px;line-height:1.75;color:var(--muted);margin:0 0 8px}.privacy .button{margin:4px 0 9px}.privacy small{display:block;font-size:11px;color:var(--dim)}noscript p{margin:24px;border:1px solid var(--line);padding:20px}
    @media(min-width:1000px){.metrics .metric:nth-child(n+4){padding-top:17px;padding-bottom:17px}.metrics .metric:nth-child(n+4) .number{font-size:30px}}@media(max-width:760px){main{padding-top:32px}.heading{align-items:flex-start;flex-direction:column;gap:22px}.heading .controls{width:100%}.metrics{gap:10px;grid-template-columns:repeat(2,minmax(0,1fr))}.metric{padding:18px}.panels{grid-template-columns:1fr}.privacy{grid-template-columns:1fr;gap:22px}.status-line{align-items:flex-start;flex-direction:column;gap:4px;margin-bottom:16px}.badge{margin-left:auto}.panel-head{padding:18px 18px 0}td,th{padding:12px 18px}}@media(max-width:400px){.brand span{display:none}.badge{display:none}.range button{padding-left:9px;padding-right:9px}.metric{padding:15px}.metric h2{font-size:11px}.metric p{font-size:10px}.chart{height:190px}}
  </style>
</head>
<body>
  <header class="topbar"><a class="brand" href="/" aria-label="Hubu home"><img src="/brand/hubu-wordmark.svg" alt="Hubu" width="80" height="26"><span>Website analytics</span></a><a class="site-link" href="/">hubustack.dev ↗</a></header>
  <main id="main">
    <div class="heading"><div><p class="eyebrow">Audience overview</p><h1>People behind the page views.</h1><p class="lede">Estimated unique browsers, with known bots and automated browser traffic filtered.</p></div><div class="controls"><div class="range" role="group" aria-label="Report range"><button type="button" data-days="1" aria-pressed="false">Today</button><button type="button" data-days="7" aria-pressed="true">7 days</button><button type="button" data-days="30" aria-pressed="false">30 days</button></div><button class="button" id="refresh" type="button">↻ Refresh</button></div></div>
    <div class="status-line"><span id="period">Loading report…</span><span class="badge">Private dashboard · UTC</span></div>
    <p class="notice" id="error" role="alert" hidden></p>
    <p class="notice" id="disabled" hidden>Collection is not enabled. Public page views are not currently recorded. Existing data, if any, remains visible below.</p>
    <p id="status" class="sr-only" role="status" aria-live="polite">Loading analytics.</p>
    <div class="empty" id="empty" hidden><strong>Your audience starts here.</strong>No accepted page views in this period. Counts begin after setup; visit a public page from an included browser to start collecting.</div>
    <div id="report" aria-busy="true">
      <section class="metrics" aria-label="Audience metrics">
        <article class="metric primary"><h2>Unique visitors</h2><div class="number" id="uniqueVisitors">—</div><p>Distinct browser identities in this period</p></article>
        <article class="metric"><h2>Visits</h2><div class="number" id="visits">—</div><p>A new visit begins after 30 minutes idle</p></article>
        <article class="metric"><h2>Page views</h2><div class="number" id="pageViews">—</div><p>Accepted browser page view events</p></article>
        <article class="metric"><h2>New visitors</h2><div class="number" id="newVisitors">—</div><p>Browser identities first seen in this period</p></article>
        <article class="metric"><h2>Returning visitors</h2><div class="number" id="returningVisitors">—</div><p>Browser identities first seen before this period</p></article>
        <article class="metric"><h2>Excluded bot events</h2><div class="number" id="excludedEvents">—</div><p>Rejected beacons; excludes bots blocked at the edge</p></article>
      </section>
      <section class="panel trend" aria-labelledby="trend-title"><div class="panel-head"><h2 id="trend-title">Daily visitors</h2><p>Distinct browsers per UTC day</p></div><div id="chart" class="chart"></div><p class="legend">Unique visitors · daily counts may overlap</p><details><summary>View daily data table</summary><div class="table-wrap"><table><caption class="sr-only">Daily analytics in UTC</caption><thead><tr><th scope="col">Date (UTC)</th><th scope="col" class="num">Visitors</th><th scope="col" class="num">Visits</th><th scope="col" class="num">Views</th><th scope="col" class="num">Excluded</th></tr></thead><tbody id="daily"></tbody></table></div></details></section>
      <div class="panels">
        <section class="panel" aria-labelledby="pages-title"><div class="panel-head"><h2 id="pages-title">Top pages</h2><p>By page views</p></div><div class="table-wrap"><table><caption class="sr-only">Top pages</caption><thead><tr><th scope="col">Path</th><th scope="col" class="num">Views</th><th scope="col" class="num">Visitors</th></tr></thead><tbody id="pages"></tbody></table></div><p class="table-empty" id="pages-empty">Waiting for data…</p></section>
        <section class="panel" aria-labelledby="referrers-title"><div class="panel-head"><h2 id="referrers-title">Referrers</h2><p>By visits</p></div><div class="table-wrap"><table><caption class="sr-only">Visit referrers</caption><thead><tr><th scope="col">Source</th><th scope="col" class="num">Visits</th><th scope="col" class="num">Visitors</th></tr></thead><tbody id="referrers"></tbody></table></div><p class="table-empty" id="referrers-empty">Waiting for data…</p></section>
        <section class="panel" aria-labelledby="countries-title"><div class="panel-head"><h2 id="countries-title">Countries</h2><p>Approximate location</p></div><div class="table-wrap"><table><caption class="sr-only">Visitor countries</caption><thead><tr><th scope="col">Country</th><th scope="col" class="num">Visitors</th></tr></thead><tbody id="countries"></tbody></table></div><p class="table-empty" id="countries-empty">Waiting for data…</p></section>
        <section class="panel" aria-labelledby="exclusions-title"><div class="panel-head"><h2 id="exclusions-title">Bot filtering</h2><p>Rejected beacon events</p></div><div class="table-wrap"><table><caption class="sr-only">Reasons analytics events were excluded</caption><thead><tr><th scope="col">Reason</th><th scope="col" class="num">Events</th></tr></thead><tbody id="exclusions"></tbody></table></div><p class="table-empty" id="exclusions-empty">Waiting for data…</p></section>
      </div>
    </div>
    <footer class="privacy"><div><h2>What these numbers mean</h2><p>A visitor is a browser with a first-party identity cookie, not a verified person. Multiple devices and cleared cookies create additional identities. Browser privacy controls, blocked scripts, and undetected bots affect these estimates.</p><p>Counts begin after setup. Query strings and URL fragments are not collected. Dates and day boundaries use UTC. Excluded events count rejected analytics beacons, not all automated requests or bots blocked by Cloudflare before reaching this collector.</p></div><div><h2>Your own traffic</h2><p id="exclusion-state">Checking this browser’s preference…</p><button class="button" id="toggle-exclusion" type="button">Exclude this browser</button><small id="exclusion-note">Applies to future events from this browser only. This dashboard never records page views.</small></div></footer>
  </main>
  <noscript><p>Enable JavaScript to load this private analytics report.</p></noscript>
  <script>
  (function () {
    'use strict';
    var days = 7;
    var requestId = 0;
    var controller = null;
    var hasReport = false;
    var dailyData = [];
    var number = new Intl.NumberFormat();
    var date = new Intl.DateTimeFormat('en', { month: 'short', day: 'numeric', year: 'numeric', timeZone: 'UTC' });
    var shortDate = new Intl.DateTimeFormat('en', { month: 'short', day: 'numeric', timeZone: 'UTC' });
    function element(id) { return document.getElementById(id); }
    function count(value) { return Number.isFinite(Number(value)) && Number(value) >= 0 ? Math.floor(Number(value)) : 0; }
    function value(value) { return value == null ? '' : String(value); }
    function rows(value) { return Array.isArray(value) ? value : []; }
    function validDate(value) { var result = new Date(value); return Number.isNaN(result.getTime()) ? null : result; }
    function formatDate(input, formatter) { var result = validDate(input); return result ? formatter.format(result) : value(input); }
    function cell(row, text, numeric) { var td = document.createElement('td'); td.textContent = text; if (numeric) td.className = 'num'; row.appendChild(td); }
    function table(id, data, columns) {
      var body = element(id);
      body.replaceChildren();
      data.forEach(function (item) { var row = document.createElement('tr'); columns.forEach(function (column) { cell(row, column[0](item), column[1]); }); body.appendChild(row); });
      var empty = element(id + '-empty');
      if (empty) { empty.hidden = data.length > 0; empty.textContent = id === 'exclusions' ? 'No bot beacons excluded in this period.' : 'No data in this period.'; }
    }
    function svgElement(tag, attributes, text) {
      var node = document.createElementNS('http://www.w3.org/2000/svg', tag);
      Object.keys(attributes || {}).forEach(function (key) { node.setAttribute(key, String(attributes[key])); });
      if (text != null) node.textContent = text;
      return node;
    }
    function chart(daily) {
      dailyData = daily;
      var width = Math.max(240, element('chart').clientWidth || 1000), height = 230, left = 48, right = 12, top = 20, bottom = 35;
      var plotWidth = width - left - right, plotHeight = height - top - bottom;
      var max = Math.max(2, Math.ceil(Math.max(0, ...daily.map(function (day) { return count(day.uniqueVisitors); })) / 2) * 2);
      var svg = svgElement('svg', { viewBox: '0 0 ' + width + ' ' + height, role: 'img', 'aria-labelledby': 'chart-description', preserveAspectRatio: 'none' });
      svg.appendChild(svgElement('title', { id: 'chart-description' }, 'Unique visitors by UTC day. The daily data table follows this chart.'));
      [0, .5, 1].forEach(function (fraction) {
        var y = top + plotHeight * (1 - fraction);
        svg.appendChild(svgElement('line', { x1: left, x2: width - right, y1: y, y2: y, class: fraction === 0 ? 'baseline' : 'grid' }));
        svg.appendChild(svgElement('text', { x: left - 12, y: y + 4, 'text-anchor': 'end' }, number.format(Math.ceil(max * fraction))));
      });
      var step = plotWidth / Math.max(1, daily.length);
      daily.forEach(function (day, index) {
        var visitors = count(day.uniqueVisitors), barHeight = plotHeight * visitors / max;
        var bar = svgElement('rect', { x: left + step * index + step * .2, y: top + plotHeight - barHeight, width: step * .6, height: barHeight, rx: Math.min(3, step * .1), class: 'bar' });
        bar.appendChild(svgElement('title', {}, value(day.date) + ': ' + number.format(visitors) + ' unique visitors'));
        svg.appendChild(bar);
        if ((daily.length <= 7 && width > 600) || index === 0 || index === Math.floor(daily.length / 2) || index === daily.length - 1) {
          svg.appendChild(svgElement('text', { x: left + step * (index + .5), y: height - 10, 'text-anchor': 'middle' }, formatDate(day.date, shortDate)));
        }
      });
      element('chart').replaceChildren(svg);
    }
    function country(code) {
      if (!code || code === 'XX') return 'Unknown';
      try { return new Intl.DisplayNames(['en'], { type: 'region' }).of(code) || code; } catch (_) { return value(code); }
    }
    function render(data) {
      if (!data || !data.summary || !Array.isArray(data.daily)) throw new Error('Invalid analytics response');
      ['uniqueVisitors', 'visits', 'pageViews', 'newVisitors', 'returningVisitors', 'excludedEvents'].forEach(function (key) { element(key).textContent = number.format(count(data.summary[key])); });
      element('period').textContent = formatDate(data.since, date) + ' – ' + formatDate(data.until, date) + ' · Updated ' + new Intl.DateTimeFormat('en', { hour: '2-digit', minute: '2-digit', timeZone: 'UTC' }).format(new Date()) + ' UTC';
      element('disabled').hidden = data.enabled !== false;
      element('empty').hidden = data.enabled === false || count(data.summary.pageViews) > 0;
      chart(data.daily);
      table('daily', data.daily, [[function (d) { return value(d.date); }, false], [function (d) { return number.format(count(d.uniqueVisitors)); }, true], [function (d) { return number.format(count(d.visits)); }, true], [function (d) { return number.format(count(d.pageViews)); }, true], [function (d) { return number.format(count(d.excludedEvents)); }, true]]);
      table('pages', rows(data.pages), [[function (d) { return value(d.path); }, false], [function (d) { return number.format(count(d.views)); }, true], [function (d) { return number.format(count(d.visitors)); }, true]]);
      table('referrers', rows(data.referrers), [[function (d) { return value(d.referrer) || 'Direct / unknown'; }, false], [function (d) { return number.format(count(d.visits)); }, true], [function (d) { return number.format(count(d.visitors)); }, true]]);
      table('countries', rows(data.countries), [[function (d) { return country(d.country); }, false], [function (d) { return number.format(count(d.visitors)); }, true]]);
      table('exclusions', rows(data.exclusions), [[function (d) { return value(d.reason).replace(/_/g, ' '); }, false], [function (d) { return number.format(count(d.count)); }, true]]);
    }
    async function load() {
      var id = ++requestId;
      if (controller) controller.abort();
      controller = new AbortController();
      element('report').setAttribute('aria-busy', 'true');
      element('refresh').disabled = true;
      element('error').hidden = true;
      element('status').textContent = 'Loading analytics for ' + days + (days === 1 ? ' day.' : ' days.');
      try {
        var response = await fetch('/_analytics/report?days=' + days, { credentials: 'same-origin', cache: 'no-store', signal: controller.signal, headers: { accept: 'application/json' } });
        if (!response.ok) throw new Error(response.status === 401 || response.status === 403 ? 'Your dashboard session has expired. Reload the page to sign in.' : 'The report could not be loaded. Try refreshing in a moment.');
        if (!(response.headers.get('content-type') || '').includes('application/json')) throw new Error('Your dashboard session may have expired. Reload the page to sign in.');
        var data = await response.json();
        if (id !== requestId) return;
        render(data);
        hasReport = true;
        element('status').textContent = (data.enabled === false ? 'Collection is disabled. ' : 'Report loaded. ') + number.format(count(data.summary.uniqueVisitors)) + ' estimated unique visitors.';
      } catch (error) {
        if (id !== requestId || error.name === 'AbortError') return;
        element('error').textContent = (error.message || 'The report could not be loaded.') + (hasReport ? ' The previous report remains below.' : '');
        element('error').hidden = false;
        if (!hasReport) element('period').textContent = 'Report unavailable';
        element('status').textContent = 'Analytics report could not be loaded.';
      } finally {
        if (id === requestId) { element('refresh').disabled = false; element('report').setAttribute('aria-busy', 'false'); }
      }
    }
    document.querySelectorAll('[data-days]').forEach(function (button) {
      button.addEventListener('click', function () {
        days = Number(button.dataset.days);
        document.querySelectorAll('[data-days]').forEach(function (item) { item.setAttribute('aria-pressed', String(item === button)); });
        load();
      });
    });
    element('refresh').addEventListener('click', load);
    window.addEventListener('resize', function () { if (hasReport) chart(dailyData); });
    function isExcluded() { return document.cookie.split(';').some(function (cookie) { return cookie.trim() === 'hubu_analytics_exclude=1'; }); }
    function updateExclusion() {
      var excluded = isExcluded();
      element('exclusion-state').textContent = excluded ? 'This browser is excluded from future analytics.' : 'This browser is included when it visits public pages.';
      element('toggle-exclusion').textContent = excluded ? 'Include this browser' : 'Exclude this browser';
    }
    element('toggle-exclusion').addEventListener('click', function () {
      var excluded = isExcluded();
      document.cookie = 'hubu_analytics_exclude=' + (excluded ? '' : '1') + '; Path=/; Max-Age=' + (excluded ? '0' : '31536000') + '; SameSite=Lax; Secure';
      updateExclusion();
      if (isExcluded() === excluded) element('exclusion-state').textContent = 'The preference could not be saved. Check that cookies are allowed and you are using HTTPS.';
    });
    updateExclusion();
    load();
  })();
  </script>
</body>
</html>`;
