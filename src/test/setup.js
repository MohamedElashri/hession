import { afterEach } from 'vitest';
import { cleanup, configure } from '@testing-library/react';

// jsdom does not implement native modal presentation. Keep the actual React
// modal lifecycle while replacing only the missing platform primitives.
HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
HTMLDialogElement.prototype.close = function () { this.removeAttribute('open'); };

configure({ asyncUtilTimeout: 5000 });
afterEach(() => cleanup());
