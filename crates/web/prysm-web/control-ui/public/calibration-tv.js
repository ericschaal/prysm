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
    closed = true;
    if (token)
      fetch('/api/calibration', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'cancel', token: token }),
        keepalive: true,
      }).catch(function () {});
    token = null;
  }
  window.addEventListener('message', function (event) {
    if (
      event.source !== window.parent ||
      (event.origin !== 'null' && event.origin.indexOf('file://') !== 0) ||
      !event.data
    )
      return;
    if (event.data.action === 'cancel') {
      cancel();
      return;
    }
    if (event.data.action !== 'poll' || busy || closed) return;
    busy = true;
    var body = token
      ? {
          action: 'poll',
          token: token,
          compare: false,
          playing: event.data.playing === true,
        }
      : { action: 'begin' };
    var request = new window.XMLHttpRequest();
    request.open('POST', '/api/calibration');
    request.setRequestHeader('Content-Type', 'application/json');
    request.responseType = 'json';
    request.timeout = 5000;
    function failed() {
      busy = false;
      if (closed) return;
      var terminal = request.status === 409;
      if (terminal) {
        closed = true;
        token = null;
      }
      send({
        type: 'calibration-error',
        terminal: terminal,
        message:
          (request.response && request.response.message) ||
          'Receiver unavailable.',
      });
    }
    request.onerror = request.ontimeout = failed;
    request.onload = function () {
      if (
        request.status !== 200 ||
        !request.response ||
        !request.response.token
      ) {
        failed();
        return;
      }
      busy = false;
      if (body.action === 'begin') token = request.response.token;
      if (closed) cancel();
      else send({ type: 'calibration', view: request.response });
    };
    request.send(JSON.stringify(body));
  });
  window.addEventListener('pagehide', cancel);
})();
