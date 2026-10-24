export function textPreview(text, maxCharacters = 1600, maxLines = 28) {
  const value = String(text ?? '');
  let end = Math.min(value.length, maxCharacters);
  let lines = 1;
  for (let index = 0; index < end; index++) {
    if (value[index] === '\n' && ++lines > maxLines) { end = index; break; }
  }
  // Avoid splitting a UTF-16 surrogate pair at the character boundary.
  if (end < value.length && end > 0 && /[\uD800-\uDBFF]/.test(value[end - 1])) end--;
  return value.slice(0, end);
}
