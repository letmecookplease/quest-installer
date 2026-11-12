import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatSize } from "./model";

interface BuildStorageProps {
  estimatedBytes: number;
  serial: string | null;
  busy: boolean;
}

export function BuildStorage({ estimatedBytes, serial, busy }: BuildStorageProps) {
  const [reading, setReading] = useState<{ serial: string; bytes: number | null } | null>(null);

  useEffect(() => {
    if (!serial || busy) return;
    let live = true;
    setReading(null);
    void invoke<number>("available_space", { serial }).then(
      (bytes) => { if (live) setReading({ serial, bytes }); },
      () => { if (live) setReading({ serial, bytes: null }); },
    );
    return () => { live = false; };
  }, [serial, busy, estimatedBytes]);

  const free = reading?.serial === serial ? reading.bytes : undefined;
  return (
    <section aria-label="Headset storage">
      <dl>
        <dt>Estimated install size</dt>
        <dd className="mono"><strong>{formatSize(estimatedBytes)}</strong></dd>
        {serial && <>
          <dt>Free on headset</dt>
          <dd className="mono">{free === undefined ? "Checking…" : free === null ? "Unavailable" : formatSize(free)}</dd>
        </>}
      </dl>
      <p className="storage-note muted">Android may need extra room during installation and for app data.</p>
      {typeof free === "number" && free < estimatedBytes && (
        <p className="storage-note storage-shortfall">This build is larger than the space currently free.</p>
      )}
    </section>
  );
}
