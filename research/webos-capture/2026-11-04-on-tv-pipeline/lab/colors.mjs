import {setTimeout as sleep} from 'node:timers/promises';
const ws = new WebSocket(new URL('/ws',process.env.PRYSM_WLED_URL).href.replace(/^http/,'ws'));
ws.binaryType = 'arraybuffer';
const levels = [];
ws.onopen = () => ws.send('{"lv":true}');
ws.onmessage = ({data}) => {
  if (typeof data === 'string') return;
  const b = new Uint8Array(data);
  if (b[0] === 76 && b[1] === 1) levels.push(b[3]);
};
await sleep(15000);
ws.close();
console.log(JSON.stringify({samples: levels.length, min: Math.min(...levels), max: Math.max(...levels)}));
