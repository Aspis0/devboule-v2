export function quotePermissionArg(value: string): string {
  if (value.length === 0 || /[\s"]/.test(value)) {
    return `"${value.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`;
  }
  return value;
}
