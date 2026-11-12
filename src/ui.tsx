import { useEffect, useRef, type ReactNode } from "react";
import type { Failure } from "./model";

export function Modal({ title, close, children }: { title: string; close: () => void; children: ReactNode }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const node = dialog.current;
    const previous = document.activeElement;
    node?.showModal();
    return () => {
      node?.close();
      if (previous instanceof HTMLElement) previous.focus();
    };
  }, []);
  return <dialog ref={dialog} aria-labelledby="dialog-title" onCancel={(event) => {
    event.preventDefault();
    close();
  }}>
    <h2 id="dialog-title">{title}</h2>
    {children}
  </dialog>;
}

export function ErrorMessage({ error, children }: { error: Failure; children?: ReactNode }) {
  return <section className="notice error" role="alert">
    <strong>{error.message}</strong>
    {error.raw && <details><summary>ADB output</summary><pre>{error.raw}</pre></details>}
    {children}
  </section>;
}
