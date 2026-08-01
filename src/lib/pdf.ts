// Tauri's WKWebView can lag the newest ECMAScript collection APIs used by PDF.js.
// The supported legacy build provides the same renderer plus its required polyfills.
import * as pdfjs from 'pdfjs-dist/legacy/build/pdf.mjs';
import workerUrl from 'pdfjs-dist/legacy/build/pdf.worker.min.mjs?url';
import type { PDFDocumentProxy } from 'pdfjs-dist';
import { api } from '$lib/api';

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

const MAX_CACHED_DOCUMENTS = 8;
type CachedDocument = {
  promise: Promise<PDFDocumentProxy>;
  destroy: () => Promise<void>;
};

const documents = new Map<string, CachedDocument>();

export function loadProjectPdf(
  projectId: number,
  revision: number
): Promise<PDFDocumentProxy> {
  const key = `${projectId}:${revision}`;
  const existing = documents.get(key);
  if (existing) {
    documents.delete(key);
    documents.set(key, existing);
    return existing.promise;
  }

  let loadingTask: ReturnType<typeof pdfjs.getDocument> | undefined;
  const promise = api.readProjectPdf(projectId).then((data) => {
    loadingTask = pdfjs.getDocument({ data });
    return loadingTask.promise;
  });
  const loading: CachedDocument = {
    promise,
    destroy: async () => {
      await promise.catch(() => undefined);
      await loadingTask?.destroy();
    }
  };
  documents.set(key, loading);
  while (documents.size > MAX_CACHED_DOCUMENTS) {
    const oldestKey = documents.keys().next().value;
    if (oldestKey === undefined) break;
    const discarded = documents.get(oldestKey);
    documents.delete(oldestKey);
    if (discarded) {
      window.setTimeout(() => {
        void discarded.destroy().catch(() => undefined);
      }, 5000);
    }
  }
  return promise;
}
