// Kills a test file's process that runs too long or grows too large, so one
// runaway file fails alone. A thread of its own watches, because the failure
// it guards against (a failed assert printing a DOM node in full) blocks the
// main thread. The runner process, which has no test context, is not watched.
// The thread samples once a second; the whole suite takes about half a minute.
import { Worker } from 'node:worker_threads';

const limitMinutes = 3;
const limitMegabytes = 3072;

if (process.env.NODE_TEST_CONTEXT) {
  new Worker(
    `
    const { writeSync } = require('node:fs');
    const { workerData: file } = require('node:worker_threads');
    const started = Date.now();
    setInterval(() => {
      const megabytes = process.memoryUsage.rss() / 2 ** 20;
      const minutes = (Date.now() - started) / 60000;
      if (megabytes < ${limitMegabytes} && minutes < ${limitMinutes}) return;
      writeSync(
        2,
        \`watchdog: \${file} killed after \${minutes.toFixed(1)} min at \${Math.round(megabytes)} MB\\n\`,
      );
      process.kill(process.pid, 'SIGKILL');
    }, 1000);
    `,
    { eval: true, workerData: process.argv[1] },
  ).unref();
}
