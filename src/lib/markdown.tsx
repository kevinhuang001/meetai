/**
 * 极简 markdown 渲染器（不引依赖）。
 * 只支持后端报告实际会产出的语法：标题、无序/有序列表、表格、粗体、行内代码、引用、分割线。
 * 直接产出 React 节点，不使用 dangerouslySetInnerHTML。
 */
import type { ReactNode } from "react";

type Block =
  | { kind: "heading"; level: number; text: string }
  | { kind: "list"; ordered: boolean; items: string[] }
  | { kind: "table"; head: string[]; rows: string[][] }
  | { kind: "quote"; text: string }
  | { kind: "paragraph"; text: string }
  | { kind: "hr" };

function splitRow(line: string): string[] {
  return line
    .replace(/^\s*\|/, "")
    .replace(/\|\s*$/, "")
    .split("|")
    .map((c) => c.trim());
}

function isSeparator(line: string): boolean {
  return /^\s*\|?[\s:-]*-[\s|:-]*\|?\s*$/.test(line) && line.includes("-");
}

export function parseMarkdown(md: string): Block[] {
  const lines = md.replace(/\r\n/g, "\n").split("\n");
  const blocks: Block[] = [];
  let i = 0;

  while (i < lines.length) {
    const raw = lines[i];
    const line = raw.trimEnd();

    if (!line.trim()) {
      i += 1;
      continue;
    }

    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      blocks.push({ kind: "heading", level: heading[1].length, text: heading[2].trim() });
      i += 1;
      continue;
    }

    if (/^\s*(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
      blocks.push({ kind: "hr" });
      i += 1;
      continue;
    }

    // 表格：当前行是 | ... |，下一行是分隔行
    if (line.trim().startsWith("|") && i + 1 < lines.length && isSeparator(lines[i + 1])) {
      const head = splitRow(line);
      const rows: string[][] = [];
      i += 2;
      while (i < lines.length && lines[i].trim().startsWith("|")) {
        rows.push(splitRow(lines[i]));
        i += 1;
      }
      blocks.push({ kind: "table", head, rows });
      continue;
    }

    if (/^\s*>\s?/.test(line)) {
      const buf: string[] = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        buf.push(lines[i].replace(/^\s*>\s?/, ""));
        i += 1;
      }
      blocks.push({ kind: "quote", text: buf.join(" ") });
      continue;
    }

    const ul = /^\s*[-*+]\s+(.*)$/.exec(line);
    const ol = /^\s*\d+[.)]\s+(.*)$/.exec(line);
    if (ul || ol) {
      const ordered = Boolean(ol);
      const items: string[] = [];
      while (i < lines.length) {
        const l = lines[i];
        const m = ordered ? /^\s*\d+[.)]\s+(.*)$/.exec(l) : /^\s*[-*+]\s+(.*)$/.exec(l);
        if (!m) break;
        items.push(m[1].trim());
        i += 1;
      }
      blocks.push({ kind: "list", ordered, items });
      continue;
    }

    const buf: string[] = [];
    while (i < lines.length && lines[i].trim() && !/^\s*(#{1,6}\s|[-*+]\s|\d+[.)]\s|>|\|)/.test(lines[i])) {
      buf.push(lines[i].trim());
      i += 1;
    }
    if (buf.length) blocks.push({ kind: "paragraph", text: buf.join(" ") });
    else i += 1;
  }

  return blocks;
}

/** 行内语法：**粗体**、`代码` */
function inline(text: string, keyPrefix: string): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /\*\*([^*]+)\*\*|`([^`]+)`/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let n = 0;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) out.push(text.slice(last, m.index));
    if (m[1] !== undefined) out.push(<strong key={`${keyPrefix}-b${n}`}>{m[1]}</strong>);
    else out.push(<code key={`${keyPrefix}-c${n}`}>{m[2]}</code>);
    last = m.index + m[0].length;
    n += 1;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

export function Markdown({ text, className }: { text: string; className?: string }): ReactNode {
  const blocks = parseMarkdown(text);
  return (
    <div className={className ? `md ${className}` : "md"} data-testid="markdown">
      {blocks.map((b, idx) => {
        const key = `b${idx}`;
        switch (b.kind) {
          case "heading": {
            const Tag = (`h${Math.min(6, Math.max(1, b.level))}` as "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
            return <Tag key={key}>{inline(b.text, key)}</Tag>;
          }
          case "list":
            return b.ordered ? (
              <ol key={key}>
                {b.items.map((it, j) => (
                  <li key={`${key}-${j}`}>{inline(it, `${key}-${j}`)}</li>
                ))}
              </ol>
            ) : (
              <ul key={key}>
                {b.items.map((it, j) => (
                  <li key={`${key}-${j}`}>{inline(it, `${key}-${j}`)}</li>
                ))}
              </ul>
            );
          case "table":
            return (
              <div className="md-table-wrap" key={key}>
                <table>
                  <thead>
                    <tr>
                      {b.head.map((h, j) => (
                        <th key={`${key}-h${j}`}>{inline(h, `${key}-h${j}`)}</th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {b.rows.map((r, j) => (
                      <tr key={`${key}-r${j}`}>
                        {r.map((c, k) => (
                          <td key={`${key}-r${j}c${k}`}>{inline(c, `${key}-r${j}c${k}`)}</td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            );
          case "quote":
            return <blockquote key={key}>{inline(b.text, key)}</blockquote>;
          case "hr":
            return <hr key={key} />;
          default:
            return <p key={key}>{inline(b.text, key)}</p>;
        }
      })}
    </div>
  );
}
