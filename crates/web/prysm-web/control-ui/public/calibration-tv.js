/* global window */
'use strict';
(function () {
  var token,
    busy = false,
    closed = false;
  function send(value) {
    window.parent.postMessage(value, '*');
  }
  function cancel() {
    if (token)
      fetch('/api/calibration', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'cancel', token: token }),
        keepalive: true,
      }).catch(function () {});
  }
  window.addEventListener('message', function (event) {
    if (
      event.source !== window.parent ||
      (event.origin !== 'null' && event.origin.indexOf('file://') !== 0) ||
      !event.data
    )
      return;
    var action = event.data.action;
    if (action === 'cancel') {
      closed = true;
      cancel();
      return;
    }
    if (
      ['begin', 'poll'].indexOf(action) < 0 ||
      busy ||
      closed ||
      (action === 'begin' && token) ||
      (action === 'poll' && !token)
    )
      return;
    busy = true;
    var body =
      action === 'begin'
        ? { action: 'begin' }
        : {
            action: 'poll',
            token: token,
            compare: false,
            playing: event.data.playing === true,
          };
    fetch('/api/calibration', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
      .then(function (response) {
        return response.json().then(function (value) {
          if (!response.ok)
            throw Error(value.message || 'Receiver unavailable.');
          return value;
        });
      })
      .then(
        function (view) {
          busy = false;
          token = view.token;
          if (closed) cancel();
          else send({ type: 'calibration', view: view });
        },
        function (error) {
          busy = false;
          send({ type: 'calibration-error', message: error.message });
        },
      );
  });
  window.addEventListener('pagehide', function () {
    closed = true;
    cancel();
  });
  send({ type: 'calibration-ready' });
})();
