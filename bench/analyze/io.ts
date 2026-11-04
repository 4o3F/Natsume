function closeConsumedFile(file: Deno.FsFile) {
  try {
    file.close();
  } catch (error) {
    if (!(error instanceof Deno.errors.BadResource)) throw error;
  }
}

/** Read UTF-8 lines with bounded buffering, optionally decompressing gzip. */
export async function* lines(path: string, gzip = false) {
  const file = await Deno.open(path);
  try {
    const bytes = gzip
      ? file.readable.pipeThrough(new DecompressionStream("gzip"))
      : file.readable;
    const reader = bytes.pipeThrough(
      new TextDecoderStream("utf-8", { fatal: true }),
    );
    let pending = "";
    for await (const chunk of reader) {
      pending += chunk;
      let start = 0;
      for (
        let end = pending.indexOf("\n");
        end !== -1;
        end = pending.indexOf("\n", start)
      ) {
        yield pending.slice(start, end).replace(/\r$/, "");
        start = end + 1;
      }
      pending = pending.slice(start);
    }
    if (pending) yield pending;
  } finally {
    // Stream consumption normally closes the file, including on cancellation.
    closeConsumedFile(file);
  }
}

export type Cell = string | number;

/** Match csv.DictWriter's UTF-8, minimal quoting and CRLF records. */
export class CsvWriter {
  private pending = "";
  private constructor(
    private writer: WritableStreamDefaultWriter<Uint8Array<ArrayBuffer>>,
    private done: Promise<void>,
    private names: readonly string[],
  ) {}

  static async open(path: string, names: readonly string[]) {
    const file = await Deno.open(path, {
      write: true,
      create: true,
      truncate: true,
    });
    const stream = new TransformStream<
      Uint8Array<ArrayBuffer>,
      Uint8Array<ArrayBuffer>
    >();
    const bytes = path.endsWith(".gz")
      ? stream.readable.pipeThrough(new CompressionStream("gzip"))
      : stream.readable;
    const done = bytes.pipeTo(file.writable);
    // Observe sink failures immediately; close() still propagates them.
    done.catch(() => {});
    const result = new CsvWriter(stream.writable.getWriter(), done, names);
    await result.row(Object.fromEntries(names.map((name) => [name, name])));
    return result;
  }

  row(row: Record<string, Cell>): Promise<void> | undefined {
    this.pending += this.names.map((name) => {
      const value = String(row[name] ?? "");
      return /[",\r\n]/.test(value)
        ? `"${value.replaceAll('"', '""')}"`
        : value;
    }).join(",") + "\r\n";
    if (this.pending.length >= 65536) return this.flush();
  }

  private async flush() {
    const chunk = this.pending;
    this.pending = "";
    await this.writer.write(new TextEncoder().encode(chunk));
  }

  async close() {
    try {
      await this.flush();
      await this.writer.close();
    } finally {
      await this.done;
    }
  }
}

function csvCells(line: string): string[] {
  const cells: string[] = [];
  let value = "", quoted = false;
  for (let i = 0; i < line.length; i++) {
    const char = line[i];
    if (char === '"') {
      if (quoted && line[i + 1] === '"') {
        value += '"';
        i++;
      } else quoted = !quoted;
    } else if (char === "," && !quoted) {
      cells.push(value);
      value = "";
    } else value += char;
  }
  if (quoted) throw new Error("Unexpected multiline CSV field");
  cells.push(value);
  return cells;
}

export async function* csvRows(path: string) {
  let names: string[] | undefined;
  for await (const line of lines(path, path.endsWith(".gz"))) {
    const cells = csvCells(line);
    if (!names) {
      names = cells;
      continue;
    }
    if (names.length !== cells.length) {
      throw new Error("Invalid dataset CSV row");
    }
    yield Object.fromEntries(names.map((name, i) => [name, cells[i]]));
  }
}
