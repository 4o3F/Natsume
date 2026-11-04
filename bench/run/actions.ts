/** Schedule restart commands on A's clock, without SSH startup delay at T. */
export function restartScript(
  actions: { at: number; kind: string }[],
  start: number,
): string {
  return '#!/bin/bash\nset -euo pipefail\nprintf "kind,planned_ts_us,before_ts_us,after_ts_us\\n"\n' +
    actions.filter((a) => a.kind === "restart").sort((a, b) => a.at - b.at).map(
      (action) => {
        const target = Math.round((start + action.at) * 1e6);
        return `pause=$(awk -v target=${target} -v now="$(date +%s%6N)" 'BEGIN {remaining=(target-now)/1000000; printf "%.6f", (remaining>0?remaining:0)}')\nsleep "$pause"\nbefore=$(date +%s%6N)\nsystemctl restart natsume-bench >&2\nafter=$(date +%s%6N)\nprintf 'restart,%s,%s,%s\\n' ${target} "$before" "$after"\n`;
      },
    ).join("");
}
