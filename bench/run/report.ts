import { csvRows } from "../analyze/io.ts";

const read = async (path: string) => JSON.parse(await Deno.readTextFile(path));
const number = (value: number | null | undefined, digits = 2) =>
  value === null || value === undefined ? "—" : value.toFixed(digits);
const ms = (value: number | null | undefined) =>
  value === null || value === undefined ? "—" : number(value / 1000);

/** Assemble the completed v2.10.3 B0/B1a/B2 experiment tables. */
export async function report(date: string) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) throw new Error("Expected YYYY-MM-DD");
  const root = "context/bench/data/results",
    directory = (name: string) => `${root}/${date}-${name}`;
  const b0 = directory("B0-short"),
    short = await read(`${b0}/analysis/summary.json`),
    shortMetrics = await read(`${b0}/metrics.json`);
  const old = await read(`${root}/2026-11-03-B0/analysis/summary.json`),
    oldResources = await read(`${root}/2026-11-03-B0/resource-summary.json`);
  const latency = (summary: typeof short, route: string, status: string) =>
    summary.windows.steady.latency_us.find((
      row: { route: string; status: string },
    ) => row.route === route && row.status === status);
  const logo = "/api/v2/organizations/{organization_id}/logo",
    devices = "/api/v2/devices";
  let text =
    `# v2.10.3 B1a 与 B2 实验结果\n\nA 运行隔离 Server，B 运行 loadgen；B 的所有负载均绑定 NUMA node 0。CPU 100% 表示一个逻辑核。普通实验使用 B0 原始 Server 二进制；CPU 归因使用同一 tag 的帧指针构建，构建参数和二进制摘要在 B1a-perf-fp/profile-build.json。每轮恢复独立 bench 基线，原始 journal、loadgen events、资源采样与分析结果保存在对应目录。\n\n| 指标 | 30 分钟 B0 | 10 分钟 B0 |\n| --- | ---: | ---: |\n| Logo 304 服务端 p50，ms | ${
      ms(latency(old, logo, "304").p50)
    } | ${ms(latency(short, logo, "304").p50)} |\n| devices 服务端 p50，ms | ${
      ms(latency(old, devices, "200").p50)
    } | ${
      ms(latency(short, devices, "200").p50)
    } |\n| 建立连接 p50 | ${oldResources.tcp_8443_established.p50} | ${shortMetrics.resource.tcp_8443_established.p50} |\n| 平均 CPU，% | ${
      number(oldResources.cpu_total_percent)
    } | ${
      number(shortMetrics.resource.cpu_total_percent.mean)
    } |\n\n短 B0 全部 418 台持续 Active，dropped_events=${shortMetrics.dropped_events}。\n\n`;
  const perf = await read(`${directory("B1a-perf-fp")}/perf-attribution.json`),
    p = perf.exclusive_percent;
  text += `CPU 采样共 ${perf.total_samples} 个等权样本：logo 路径 ${
    number(perf.logo_inclusive_percent)
  }%，其中图像解码 ${number(p.image_decode)}%、SHA-256 ${
    number(p.sha256)
  }%、其他 logo 工作 ${number(p.other_logo)}%；其他代码 ${
    number(p.other)
  }%，未解析 ${
    number(p.unresolved)
  }%。火焰图为 [flamegraph.svg](${date}-B1a-perf-fp/flamegraph.svg)，折叠栈和原始帧指针采样与它同目录。\n\n| 实验 | 状态 / 样本数 | 客户端 p50 / p99 / max，ms | 服务端 p50 / p99 / max，ms | 服务端峰值并发 | CPU 均值 / 峰值，% |\n| --- | --- | --- | --- | ---: | ---: |\n`;
  const burstNames = ["B1a-restart-final", "B1a-cold", "B1a-contention-final"];
  const bursts = [];
  for (const name of burstNames) {
    const b = await read(`${directory(name)}/burst.json`);
    const m = await read(`${directory(name)}/metrics.json`);
    bursts.push(b);
    if (!b.full_120_seconds_observed) {
      throw new Error(`${name} lacks a full 120-second window`);
    }
    const status = name === "B1a-cold" ? "200" : "304",
      client = b.logo_latency_us[status],
      server = b.server_logo_latency_us[status];
    text += `| ${name} | ${status} / ${server.count} | ${ms(client.p50)} / ${
      ms(client.p99)
    } / ${ms(client.max)} | ${ms(server.p50)} / ${ms(server.p99)} / ${
      ms(server.max)
    } | ${b.peak_server_logo_in_flight} | ${
      number(m.resource.cpu_total_percent.mean)
    } / ${number(m.resource.cpu_total_percent.max)} |\n`;
  }
  text +=
    "\nimage_work 有 4 个许可。并发数由请求完成时间与耗时重建，包含许可前后的处理；突发时的高分位时延用于观察短时排队。\n\n| 重连 | HTTP 101 数 | p50 / p99 / max，s |\n| --- | ---: | --- |\n";
  const seconds = (r: { p50: number; p99: number; max: number }) =>
    `${number(r.p50 / 1e6, 3)} / ${number(r.p99 / 1e6, 3)} / ${
      number(r.max / 1e6, 3)
    }`;
  for (const f of bursts[0].field) {
    text += `| 现场 ${f.process} | ${f.delay_us.count} | ${
      seconds(f.delay_us)
    } |\n`;
  }
  for (const [index, name] of burstNames.entries()) {
    if (name !== "B1a-cold") {
      text += `| ${name} | ${bursts[index].reconnect_us.count} | ${
        seconds(bursts[index].reconnect_us)
      } |\n`;
    }
  }
  const contention = bursts[2],
    duration = contention.server_export_duration_us.p50;
  text += `\n导出在重启命令后 ${
    number(contention.export_after_restart_seconds, 3)
  } s 实际进入 handler；服务端导出耗时 ${
    number(duration / 1e6, 3)
  } s，现场为 4.95 s，差值 ${
    number(duration / 1e6 - 4.95, 3)
  } s。客户端完整读取耗时 ${
    number(contention.export_duration_us.p50 / 1e6, 3)
  } s。\n\n每档 ramp 5 s、预热 120 s，再测 600 s；面板固定 2 个，采用 B0 的独立路由频率。N=300 使用 418 队伍基线的前 300 台，其他档使用各自的 N 台注册基线。\n\n| N | Logo | Active min / p50 / max | 连接 p50 / max | TLS 失败 | TCP refused / timeout | Active 断线 / 成功重连 | CPU 均值，% | RSS 峰值，MiB |\n| ---: | --- | --- | --- | ---: | --- | ---: | ---: | ---: |\n`;
  const rows = [];
  for (
    const [n, on] of [
      [300, true],
      [418, true],
      [500, true],
      [600, true],
      [800, true],
      [600, false],
      [800, false],
    ] as const
  ) {
    const name = `B2-N${n}-${on ? "on" : "off"}`,
      base = directory(name),
      w = await read(`${base}/window.json`),
      m = await read(`${base}/metrics.json`);
    if (w.status !== "completed" || m.dropped_events !== 0) {
      throw new Error(`${name} did not complete cleanly`);
    }
    const failures = m.connection_failures ??
      Object.entries(m.event_counts).filter(([key]) =>
        key.startsWith("tcp_tls:") && key !== "tcp_tls:OK"
      ).reduce((sum, [, v]) => sum + Number(v), 0);
    rows.push({ name, n, on, w, m, failures });
    text += `| ${n} | ${
      on ? "ON" : "OFF"
    } | ${m.active.min} / ${m.active.p50} / ${m.active.max} | ${m.resource.tcp_8443_established.p50} / ${m.resource.tcp_8443_established.max} | ${failures} | ${
      m.event_counts["tcp_tls:TCP_REFUSED"] ?? 0
    } / ${m.event_counts["tcp_tls:TIMEOUT"] ?? 0} | ${
      m.active_disconnects ?? 0
    } / ${m.reconnect_successes ?? 0} | ${
      number(m.resource.cpu_total_percent.mean)
    } | ${number(m.resource.rss_bytes.max / 1048576)} |\n`;
  }
  text +=
    "\nTLS 失败按 tcp_tls 阶段每次失败尝试计数；Active 断线只统计已完成握手的设备失联，成功重连统计同一设备后续的成功握手。具体 TCP_RESET、TLS_EOF、HTTPS_CONNECT 等类别在每轮 metrics.json 中。\n\n| N / Logo | 已有面板 devices 200 数 / p99 ms | 新面板登录成功 / 尝试 | 新面板 devices 成功数 |\n| --- | --- | --- | ---: |\n";
  for (const row of rows) {
    let logins = 0, success = 0, deviceSuccess = 0;
    for await (
      const event of csvRows(`${directory(row.name)}/probe-events.csv.gz`)
    ) {
      if (event.event_kind === "operator_login") {
        logins++;
        if (event.detail_code === "200") success++;
      }
      if (
        event.event_kind === "operator_devices" && event.detail_code === "200"
      ) deviceSuccess++;
    }
    const existing = row.m.latency_us["operator_devices:200"];
    text += `| ${row.n} / ${row.on ? "ON" : "OFF"} | ${
      existing?.count ?? 0
    } / ${ms(existing?.p99)} | ${success} / ${logins} | ${deviceSuccess} |\n`;
  }
  const knee = rows.find((row) =>
    row.on &&
    (row.m.active.min < row.n || row.failures > 0 ||
      Object.values(row.m.errors).some((count) => Number(count) > 0))
  );
  text += `\n首次出现接入损失的测量档为 ${
    knee?.n ?? "未出现"
  } 台。v2.10.3 server/src/tls.rs 的 MAX_CONNECTIONS=1024，许可覆盖 TLS、HTTP keep-alive 与升级后的 WebSocket；logo ON 的连接预算约为 2N+2，511 台对应 1024 条，512 台开始超过预算。600/800 台的 ON/OFF 对照中，连接数、可用设备数和新面板准入的变化共同定位连接许可限制；具体观察值见上表。\n\n每轮执行 bench/analyze 的 extract、summarize、series，并核对原始 journal 与提取结果的 HTTP 计数。资源样本和负载事件保留原始时钟，汇总使用记录的时差校正。CPU 采样的符号展开、分号规范化和 SVG 浏览器渲染已检查。较早的 DWARF 排查、采样器中断轮及注入时序先导轮保留在结果目录中；本表使用 final 复测窗口。\n\n工具验证：cargo test -p natsume-loadgen；cargo clippy -p natsume-loadgen --all-targets -- -D warnings；deno task check scripts；deno task test scripts；prek run --all-files。全量 hook 的依赖策略检查报告已有 calamine 0.37.0 的 RUSTSEC-2026-0436，具体检查结果见 ${date}-tooling-validation.md。\n\n实验结束后的单元状态记录在 ${date}-final-service-state.txt。\n`;
  const path = `${root}/${date}-B1a-B2-summary.md`;
  await Deno.writeTextFile(path, text);
  console.log(path);
}
if (import.meta.main) await report(Deno.args[0] ?? "2026-11-03");
