export const TERMINAL_ARGUMENTS = {
  generic: ['-e', 'sh', '{script}'],
  gnome: ['--', 'sh', '{script}'],
  wezterm: ['start', '--', 'sh', '{script}'],
  xfce: ['--execute', 'sh', '{script}'],
};

export function customTerminalArguments(text) {
  // One argument per line: spaces and quotes within an argument are literal.
  const args = text.split(/\r?\n/).filter(line => line.length > 0);
  if (args.filter(arg => arg === '{script}').length !== 1) {
    throw new Error('Include {script} exactly once on its own line.');
  }
  return args;
}
