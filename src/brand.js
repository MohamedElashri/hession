// Product identity from brand.json; import from here instead of spelling the name.
import brand from '../brand.json';

export const BRAND = brand;
/** Namespaced localStorage key, e.g. storageKey('theme'). */
export const storageKey = (name) => `${brand.slug}-${name}`;
/** Fired once the React app has painted, so the boot screen can stand down. */
export const READY_EVENT = `${brand.slug}-ready`;
