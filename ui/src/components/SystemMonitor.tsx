// Sidebar system monitor: CPU / GPU / memory meters and expandable details
// (network, disk I/O, temperatures). Toggled from the sidebar toolbar.
// Colours: the first three slots of the validated categorical palette (blue, orange,
// aqua), stepped per theme in app.css (--sm-cpu / --sm-gpu / --sm-mem). Values and
// labels use text tokens, so no reading depends on colour alone.
import { useEffect, useMemo, useState } from "react";
import { api } from "../api/ipc";
import type { SysStats } from "../api/types";
import { formatSize } from "../core/format";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { Icon } from "./Icon";

const POLL_MS = 2000;

// Module-level so the last sample survives the sidebar being hidden and shown again.
let latest: SysStats | null = null;
const listeners = new Set<() => void>();
let timer: number | null = null;

async function tick() {
  if (document.hidden) return;
  try {
    const s = await api.systemStats();
    latest = s;
    listeners.forEach((f) => f());
  } catch {
    /* backend unavailable (plain browser) */
  }
}

function useStats(enabled: boolean) {
  const [, force] = useState(0);
  useEffect(() => {
    if (!enabled) return;
    const f = () => force((n) => n + 1);
    listeners.add(f);
    if (timer === null) {
      tick();
      timer = window.setInterval(tick, POLL_MS);
    }
    return () => {
      listeners.delete(f);
      if (!listeners.size && timer !== null) {
        clearInterval(timer);
        timer = null;
      }
    };
  }, [enabled]);
  return latest;
}

const rate = (bps: number | null | undefined) => (bps == null ? "–" : `${formatSize(Math.round(bps))}/s`);
const gb = (b: number) => b / 1024 ** 3;
/** Compact "9.8/15 GB" so it fits under a meter in a narrow sidebar. */
const memText = (used: number, total: number) => `${gb(used).toFixed(gb(used) < 10 ? 1 : 0)}/${gb(total).toFixed(0)} GB`;
const temp = (c: number | null) => (c == null ? null : `${Math.round(c)} °C`);

/** A ring meter: temperature above it (`top`; the row is kept, empty, when another meter
 *  has one so the rings line up), label and details under it. */
function Meter({ label, note, value, color, top, topRow, sub, hint }: {
  label: string;
  /** Small text after the label: "GPU (VRAM)" says what the memory under it is. */
  note?: string;
  value: number | null; color: string; top?: string | null; topRow: boolean; sub?: string | null;
  /** What the numbers mean, shown on hover. */
  hint?: string;
}) {
  const r = 22, c = 2 * Math.PI * r;
  const v = value == null ? 0 : Math.max(0, Math.min(100, value));
  const now = value == null ? `${label}: not available` : `${label}: ${v.toFixed(1)} %`;
  return (
    <div className="sm-meter" title={[now, hint].filter(Boolean).join("\n")}>
      {topRow && <div className="sm-temp">{top ?? " "}</div>}
      <div className="sm-ring">
      <svg viewBox="0 0 56 56" aria-hidden>
        <circle cx="28" cy="28" r={r} fill="none" stroke={color} strokeOpacity="0.18" strokeWidth="5" />
        <circle cx="28" cy="28" r={r} fill="none" stroke={color} strokeWidth="5" strokeLinecap="round"
          strokeDasharray={`${(v / 100) * c} ${c}`} transform="rotate(-90 28 28)" style={{ transition: "stroke-dasharray 400ms var(--ease)" }} />
      </svg>
      <div className="sm-val">{value == null ? "–" : `${Math.round(v)}%`}</div>
      </div>
      <div className="sm-lbl">{label}{note && <span className="sm-note"> ({note})</span>}</div>
      <div className="sm-sub">{sub ?? " "}</div>
    </div>
  );
}

export function SystemMonitor() {
  const enabled = useStore((s) => s.settings?.showSystemMonitor ?? true);
  const s = useStats(enabled);
  const [open, setOpen] = useState(() => localStorage.getItem("sysmon:details") === "1");
  const showGpu = s?.gpu != null;
  // Temperatures go above the rings; no row for them where there are no sensors.
  const topRow = s?.cpuTemp != null || (showGpu && s?.gpuTemp != null);
  const memPct = s && s.memTotal ? (s.memUsed / s.memTotal) * 100 : null;
  const subtitle = useMemo(() => [s?.os.replace(/^Linux \((.*)\)$/, "$1"), s?.cpuModel.replace(/ with .*$/, "")].filter(Boolean).join(" · "), [s?.os, s?.cpuModel]);
  if (!enabled) return null;

  const menu = (e: React.MouseEvent) => A.showMenu(e.clientX, e.clientY, [
    { label: "Hide system monitor", run: () => A.updateSettings({ showSystemMonitor: false }) },
  ]);

  return (
    <div className="sysmon group">
      {/* Same head as the Drives / History groups, so it reads as part of the sidebar. */}
      <div className="group-head">
        <span className="sm-dot" title="Live" />
        <span className="title" title={[s?.host, subtitle].filter(Boolean).join("\n")}>{s?.host || "System"}</span>
        <button className="icon-btn" title="Monitor options" onClick={menu}><Icon name="more" /></button>
      </div>
      <div className="sm-meters">
        <Meter label="CPU" value={s?.cpu ?? null} color="var(--sm-cpu)" topRow={topRow} top={temp(s?.cpuTemp ?? null)} sub={s ? `${s.cpuCores} threads` : null}
          hint="Processor load over all cores; below: how many threads it runs at once" />
        {showGpu && (
          <Meter label="GPU" value={s!.gpu} color="var(--sm-gpu)" topRow={topRow} top={temp(s!.gpuTemp)}
            note={s!.gpuMemUsed != null && s!.gpuMemTotal ? "VRAM" : undefined}
            sub={s!.gpuMemUsed != null && s!.gpuMemTotal ? memText(s!.gpuMemUsed, s!.gpuMemTotal) : null}
            hint={"Graphics card load (its busiest engine); below: its own video memory (VRAM) in use / in total.\nA laptop's graphics card that is not in use rests at 0 %."} />
        )}
        <Meter label="Memory" value={memPct} color="var(--sm-mem)" topRow={topRow} sub={s ? memText(s.memUsed, s.memTotal) : null}
          hint="Main memory (RAM) in use / in total" />
      </div>
      <button className="sm-toggle" onClick={() => { setOpen(!open); localStorage.setItem("sysmon:details", open ? "0" : "1"); }}>
        <Icon name={open ? "chevronDown" : "chevronRight"} size={12} /> Network and disk
      </button>
      {open && s && (
        <div className="sm-details">
          <div><span>Network</span><b>↓ {rate(s.netRx)}  ↑ {rate(s.netTx)}</b></div>
          <div><span>Disk I/O</span><b>R {rate(s.diskRead)}  W {rate(s.diskWrite)}</b></div>
        </div>
      )}
    </div>
  );
}
