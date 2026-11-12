import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';

// jsdom does not implement native modal presentation. Keep the actual React
// modal lifecycle while replacing only the missing platform primitives.
HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
HTMLDialogElement.prototype.close = function () { this.removeAttribute('open'); };

afterEach(() => cleanup());
