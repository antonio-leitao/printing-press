import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { Worker } from 'node:worker_threads';
import {
  MAX_ZOOM,
  MIN_ZOOM,
  normalizeZoom,
  wheelZoomFactor,
  zoomBySteps
} from '../src/lib/pdf-controls.ts';
import { MAX_CANVAS_PIXELS, startPageRender } from '../src/lib/pdf-render.ts';

test('uses the PDF.js compatibility build in both the view and worker', async () => {
  const source = await readFile(new URL('../src/lib/pdf.ts', import.meta.url), 'utf8');
  assert.match(source, /pdfjs-dist\/legacy\/build\/pdf\.mjs/);
  assert.match(source, /pdfjs-dist\/legacy\/build\/pdf\.worker\.min\.mjs\?url/);

  await import('pdfjs-dist/legacy/build/pdf.mjs');
  assert.equal(typeof Map.prototype.getOrInsert, 'function');
  assert.equal(typeof Map.prototype.getOrInsertComputed, 'function');

  const workerResult = await new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./pdf-legacy-worker-probe.mjs', import.meta.url));
    worker.once('message', resolve);
    worker.once('error', reject);
  });
  assert.deepEqual(workerResult, {
    getOrInsert: 'function',
    getOrInsertComputed: 'function'
  });
});

test('passes HiDPI scaling through the PDF.js render transform', async () => {
  let parameters;
  const task = {
    promise: Promise.resolve(),
    cancel() {}
  };
  const page = {
    getViewport: ({ scale }) => ({
      width: 100 * scale,
      height: 200 * scale
    }),
    render: (received) => {
      parameters = received;
      return task;
    }
  };
  const canvas = { width: 0, height: 0, style: {} };

  const started = startPageRender(page, canvas, 1.5, 2);
  await started.task.promise;

  assert.equal(started.width, 150);
  assert.equal(started.height, 300);
  assert.equal(canvas.width, 300);
  assert.equal(canvas.height, 600);
  assert.equal(canvas.style.width, '150px');
  assert.equal(canvas.style.height, '300px');
  assert.equal(parameters.canvas, canvas);
  assert.deepEqual(parameters.transform, [2, 0, 0, 2, 0, 0]);
  assert.equal(parameters.canvasContext, undefined);
});

test('bounds viewer zoom and keeps wheel direction natural', () => {
  assert.equal(normalizeZoom(0), MIN_ZOOM);
  assert.equal(normalizeZoom(100), MAX_ZOOM);
  assert.equal(zoomBySteps(1, 1), 1.1);
  assert.equal(zoomBySteps(1, -1), 0.91);
  assert.ok(wheelZoomFactor(-10) > 1);
  assert.ok(wheelZoomFactor(10) < 1);
  assert.equal(wheelZoomFactor(-100), wheelZoomFactor(-10));
  assert.equal(wheelZoomFactor(100), wheelZoomFactor(10));
});

test('caps large zoom canvases to a safe pixel budget', () => {
  let parameters;
  const page = {
    getViewport: ({ scale }) => ({ width: 1000 * scale, height: 1000 * scale }),
    render: (received) => {
      parameters = received;
      return { promise: Promise.resolve(), cancel() {} };
    }
  };
  const canvas = { width: 0, height: 0, style: {} };

  startPageRender(page, canvas, 5, 2);

  assert.ok(canvas.width * canvas.height <= MAX_CANVAS_PIXELS);
  assert.ok(parameters.transform[0] < 1);
  assert.equal(canvas.style.width, '5000px');
  assert.equal(canvas.style.height, '5000px');
});
