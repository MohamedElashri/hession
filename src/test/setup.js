import { afterEach } from 'vitest';
import { cleanup, configure } from '@testing-library/react';

// Lazy screens are transformed on first use in each DOM worker. Keep the
// asynchronous assertion bounded without coupling it to a one-second cold
// module transform on a host also compiling native integration fixtures.
configure({ asyncUtilTimeout: 5000 });

// jsdom does not implement native modal presentation. Keep the actual React
// modal lifecycle while replacing only the missing platform primitives.
HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
HTMLDialogElement.prototype.close = function () { this.removeAttribute('open'); };

afterEach(() => cleanup());
