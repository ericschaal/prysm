/* global document, window */
'use strict';
var video = document.getElementById('pattern');
var menu = document.getElementById('menu');
var statusText = document.getElementById('status');
var generation = 0;
function stop(message) {
  generation += 1;
  video.pause();
  video.removeAttribute('src');
  video.load();
  menu.hidden = false;
  statusText.textContent = message || 'Playback ended. No settings changed.';
  document.getElementById('start').focus();
}
function play(name) {
  var current = ++generation;
  video.src = 'calibration/' + name + '.mp4';
  menu.hidden = true;
  var result = video.play();
  if (result && result.catch)
    result.catch(function () {
      if (current === generation)
        stop('Video could not play. Exit and report this result.');
    });
}
document.getElementById('start').onclick = function () {
  play('response');
};
document.getElementById('gray').onclick = function () {
  play('gray-192');
};
document.getElementById('exit').onclick = function () {
  stop();
  window.close();
};
video.onerror = function () {
  if (video.hasAttribute('src')) stop('Video decoding failed.');
};
window.addEventListener('pagehide', function () {
  stop();
});
document.addEventListener('visibilitychange', function () {
  if (document.hidden) stop();
});
document.addEventListener('keydown', function (event) {
  if (event.keyCode === 461 || event.keyCode === 27) {
    event.preventDefault();
    if (menu.hidden) stop();
    else window.close();
  } else if (!menu.hidden && [37, 38, 39, 40].indexOf(event.keyCode) !== -1) {
    event.preventDefault();
    var buttons = Array.prototype.slice.call(menu.querySelectorAll('button'));
    var offset = event.keyCode === 37 || event.keyCode === 38 ? -1 : 1;
    buttons[
      (buttons.indexOf(document.activeElement) + offset + buttons.length) %
        buttons.length
    ].focus();
  }
});
document.getElementById('start').focus();
