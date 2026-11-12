import { useEffect, useRef, useState } from "react";
import { formatSize, type Progress } from "./model";

export function InstallProgress({ progress }: { progress: Progress }) {
  const panel = useRef<HTMLElement>(null);
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    panel.current?.scrollIntoView({ block: "nearest" });
    const started = performance.now();
    const timer = window.setInterval(() => {
      setSeconds(Math.floor((performance.now() - started) / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, []);

  const elapsed = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  const { transfer } = progress;
  const percent = transfer ? Math.floor(100 * transfer.sent / transfer.total) : null;
  return (
    <section ref={panel} className="notice install-progress" aria-label="Install progress">
      <div className="install-progress-heading">
        <strong role="status">{progress.stage}</strong>
        <small className="mono">{elapsed} elapsed</small>
      </div>
      <progress
        aria-label={transfer ? "APK transfer" : "Waiting for current step"}
        value={transfer?.sent}
        max={transfer?.total ?? 1}
      />
      <div className="install-progress-meta mono muted">
        {progress.step > 0 && <span>Step {progress.step} of {progress.total}</span>}
        {transfer && <span>{formatSize(transfer.sent)} / {formatSize(transfer.total)} sent · {percent}%</span>}
      </div>
    </section>
  );
}
