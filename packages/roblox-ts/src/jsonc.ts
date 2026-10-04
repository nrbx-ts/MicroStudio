// json with comments and trailing commas, which is what a tsconfig is

// strips // and /* */ comments outside strings
function stripComments(text: string): string {
  let out = "";
  let index = 0;
  while (index < text.length) {
    const char = text[index];
    if (char === '"') {
      out += char;
      index += 1;
      while (index < text.length) {
        const inner = text[index];
        out += inner;
        if (inner === "\\") {
          out += text[index + 1] ?? "";
          index += 2;
          continue;
        }
        index += 1;
        if (inner === '"') {
          break;
        }
      }
      continue;
    }
    if (char === "/" && text[index + 1] === "/") {
      const end = text.indexOf("\n", index);
      index = end === -1 ? text.length : end;
      continue;
    }
    if (char === "/" && text[index + 1] === "*") {
      const end = text.indexOf("*/", index + 2);
      index = end === -1 ? text.length : end + 2;
      continue;
    }
    // a comma before } or ] is tolerated by typescript, not by JSON.parse
    if (char === ",") {
      let next = index + 1;
      while (next < text.length && /\s/.test(text[next] ?? "")) {
        next += 1;
      }
      if (text[next] === "}" || text[next] === "]") {
        index += 1;
        continue;
      }
    }
    out += char;
    index += 1;
  }
  return out;
}

export function parseJsonc(text: string): unknown {
  const withoutBom = text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
  return JSON.parse(stripComments(withoutBom));
}
