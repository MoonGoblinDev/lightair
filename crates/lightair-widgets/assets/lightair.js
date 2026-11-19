// Lightair's widget bridge: the page's line to Lightair's events, settings and storage.
//   lightair.on('load', fn)     settings, session, recent events and storage are ready
//   lightair.on('event', fn)    every event (chat messages included)
//   lightair.on('alert', fn)    the alert box's queue: call lightair.done() when it has gone
//   lightair.on('session'|'fields'|'button'|'storage', fn)
(function () {
  var m = location.pathname.match(/^\/w\/([^/]+)/);
  var id = m ? m[1] : '';
  var key = new URLSearchParams(location.search).get('key') || '';
  var handlers = {}, pointsRef = 0, pointsWaiting = {};
  var ws = null;
  var api = (window.lightair = {
    id: id,
    fields: {},
    session: {},
    storage: {},
    recents: [],
    data: {},
    loaded: false,
    on: function (name, fn) {
      (handlers[name] = handlers[name] || []).push(fn);
      if (name === 'load' && api.loaded) fn(api);
      return api;
    },
    done: function () { send({ type: 'done' }); },
    // runs this page's handlers as if Lightair had sent it (trying a widget out)
    emit: function (name, data) { emit(name, data); },
    // gives (delta > 0) or takes a viewer's points; cb(ok, balance): not ok when they haven't
    // enough to take
    points: function (platform, id, name, delta, cb) {
      var ref = ++pointsRef; pointsWaiting[ref] = cb || function () {};
      send({ type: 'points', platform: platform, id: id, name: name, delta: delta, ref: ref });
    },
    store: function (k, v) { api.storage[k] = v; send({ type: 'storage.set', key: k, value: v }); },
    escape: function (s) {
      return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) {
        return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
      });
    },
    // a message's fragments as HTML (emotes as pictures, never the sender's own HTML)
    render: function (e, size) {
      var h = size || '1.6em';
      if (!e.fragments || !e.fragments.length) return api.escape(e.text || '');
      return e.fragments.map(function (f) {
        if (f.type === 'emote') return '<img class="emote" alt="' + api.escape(f.code) + '" src="' + api.escape(f.image) + '" style="height:' + h + ';vertical-align:middle">';
        if (f.type === 'mention') return '<b>' + api.escape(f.name) + '</b>';
        if (f.type === 'link') return '<span class="link">' + api.escape(f.url) + '</span>';
        return api.escape(f.text);
      }).join('');
    },
    icon: function (platform) { return '/icons/' + encodeURIComponent(platform) + '.svg'; },
    // a setting naming a file on this machine, as an address the page can load
    file: function (v) {
      if (!v) return '';
      if (/^(https?:|data:|blob:)/.test(v)) return v;
      return '/w/' + id + '/file?key=' + encodeURIComponent(key) + '&path=' + encodeURIComponent(v);
    },
    // "{name} gave {amount}" with an event's values
    format: function (template, e) {
      var vals = {
        name: e.user ? e.user.display_name : '',
        amount: e.paid ? e.paid.display : (e.kind && e.kind.viewers != null ? e.kind.viewers : ''),
        viewers: e.kind && e.kind.viewers != null ? e.kind.viewers : '',
        message: e.text || '',
        headline: e.headline || '',
        platform: e.platform || ''
      };
      return String(template || '').replace(/\{(\w+)\}/g, function (all, k) { return k in vals ? vals[k] : all; });
    },
    kindOf: function (e) {
      var t = e.kind && e.kind.type;
      if (t === 'follow') return 'follow';
      if (t === 'membership') return 'sub';
      if (t === 'raid') return 'raid';
      if (t === 'paid') return e.paid && e.paid.currency === 'BITS' ? 'cheer' : 'paid';
      if (t === 'message') return 'message';
      return t || '';
    }
  });
  function emit(name, data) {
    (handlers[name] || []).forEach(function (fn) {
      try { fn(data); } catch (err) { console.error(err); }
    });
  }
  function send(msg) { if (ws && ws.readyState === 1) ws.send(JSON.stringify(msg)); }
  function connect() {
    ws = new WebSocket('ws://' + location.host + '/w/' + id + '/ws?key=' + encodeURIComponent(key));
    ws.onmessage = function (ev) {
      var msg;
      try { msg = JSON.parse(ev.data); } catch (e) { return; }
      switch (msg.type) {
        case 'load':
          api.fields = msg.fields || {};
          api.session = msg.session || {};
          api.storage = msg.storage || {};
          api.recents = msg.recents || [];
          api.data = msg.data || {};
          var first = !api.loaded;
          api.loaded = true;
          emit(first ? 'load' : 'reload', api);
          break;
        case 'fields': api.fields = msg.fields || {}; emit('fields', api.fields); break;
        case 'reload': location.reload(); break;
        case 'session': api.session = msg.session || {}; emit('session', api.session); break;
        case 'storage': api.storage = msg.storage || {}; emit('storage', api.storage); break;
        case 'event': emit('event', msg.event); break;
        case 'alert': emit('alert', msg.event); break;
        case 'button': emit('button', msg.name); break;
        case 'skip': emit('skip'); break;
        case 'speak': emit('speak', msg.text); break;
        case 'data': api.data[msg.key] = msg.value; emit('data', { key: msg.key, value: msg.value }); break;
        case 'sound': emit('sound', msg); break;
        case 'points': var cb = pointsWaiting[msg.ref]; delete pointsWaiting[msg.ref]; if (cb) cb(msg.ok, msg.balance); break;
      }
    };
    ws.onclose = function () { setTimeout(connect, 1000); };
  }
  connect();
})();
