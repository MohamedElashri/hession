import { libraryAsMarkdown } from './export.js';
self.onmessage = ({ data: { data, format } }) => {
  try {
    self.postMessage({ contents: format === 'json' ? JSON.stringify(data, null, 2) + '\n' : libraryAsMarkdown(data) });
  } catch (error) {
    self.postMessage({ error: error.message || String(error) });
  }
};
