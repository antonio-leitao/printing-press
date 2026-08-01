import { parentPort } from 'node:worker_threads';

await import('pdfjs-dist/legacy/build/pdf.worker.mjs');
parentPort?.postMessage({
  getOrInsert: typeof Map.prototype.getOrInsert,
  getOrInsertComputed: typeof Map.prototype.getOrInsertComputed
});
