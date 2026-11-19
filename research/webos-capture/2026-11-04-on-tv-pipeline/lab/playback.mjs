import fs from 'node:fs';
const mode=process.argv[2];
const pages=await (await fetch('http://127.0.0.1:28387/json/list')).json();
const page=pages.find(x=>x.description==='io.prysm.capture');
if(!page)throw Error('TV Prysm app not foreground');
const ws=new WebSocket(page.webSocketDebuggerUrl.replace(/:\d+\/devtools/,':28387/devtools'));
await new Promise((resolve,reject)=>{ws.onopen=resolve;ws.onerror=reject});
const src='data:video/mp4;base64,'+fs.readFileSync(process.env.PRYSM_CLIP).toString('base64');
const expr = {
  start:`(async()=>{const v=document.createElement('video');v.id='prysm-local-only-video';v.style.cssText='position:fixed;inset:0;width:100vw;height:100vh;z-index:2147483647;object-fit:fill';v.muted=true;v.loop=true;v.src=${JSON.stringify(src)};document.body.appendChild(v);await v.play();return v.currentSrc.slice(0,20)})()`,
  check:"(()=>{let v=document.getElementById('prysm-local-only-video');if(!v||v.paused||v.readyState<2)throw Error('benchmark video stopped');return {time:v.currentTime,width:v.videoWidth,height:v.videoHeight}})()",
  reset:"(async()=>{let v=document.getElementById('prysm-local-only-video');v.currentTime=0;await v.play();return v.readyState})()",
  stop:"document.getElementById('prysm-local-only-video')?.remove()"
}[mode];
const result=await new Promise(resolve=>{ws.onmessage=e=>resolve(JSON.parse(e.data));ws.send(JSON.stringify({id:1,method:'Runtime.evaluate',params:{expression:expr,awaitPromise:true,returnByValue:true}}))});
ws.close();
if(result.error||result.result.exceptionDetails)throw Error(JSON.stringify(result).slice(0,900));
console.log(JSON.stringify(result.result.result.value));
