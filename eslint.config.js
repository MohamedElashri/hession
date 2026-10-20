// Frontend lint rules (`make lint-js` / `npm run lint`).
import js from '@eslint/js';
import react from 'eslint-plugin-react';
import reactHooks from 'eslint-plugin-react-hooks';
import globals from 'globals';

export default [
  { ignores: ['dist/', 'node_modules/', 'src-tauri/', 'src/dev/fixtures.json'] },
  js.configs.recommended,
  {
    files: ['src/**/*.{js,jsx}'],
    languageOptions: {
      ecmaVersion: 'latest',
      sourceType: 'module',
      globals: globals.browser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    plugins: { react, 'react-hooks': reactHooks },
    settings: { react: { version: 'detect' } },
    rules: {
      ...react.configs.recommended.rules,
      ...react.configs['jsx-runtime'].rules,
      ...reactHooks.configs.recommended.rules,
      'react/prop-types': 'off',
      'no-unused-vars': ['error', { ignoreRestSiblings: true, caughtErrors: 'none' }],
    },
  },
  {
    files: ['scripts/**/*.mjs', 'vite.config.js', '.github/scripts/**/*.mjs', 'eslint.config.js'],
    languageOptions: { globals: globals.node },
  },
];
