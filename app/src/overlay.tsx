import { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { DEFAULT_SETTINGS, fromRustSettings, Settings } from "./types";
import "./overlay.css";

type Cue = { zh: string; en: string; index: number; total: number; page: number; visible: boolean };

function Overlay() {
  const [cue, setCue] = useState<Cue | null>(null);
  const [s, setS] = useState<Settings>(DEFAULT_SETTINGS);

  useEffect(() => {
    const un: Array<() => void> = [];
    (async () => {
      un.push(
        await listen<Cue>("cue", (e) => setCue(e.payload)),
        await listen<any>("style", (e) => setS(fromRustSettings(e.payload)))
      );
    })();
    return () => un.forEach((f) => f());
  }, []);

  const empty = !cue || !cue.visible || (!cue.zh && !cue.en);

  return (
    <div className="overlay-root">
      <div
        className={`bar ${empty ? "empty" : ""}`}
        data-align={s.align}
        style={{
          background: `rgba(10, 14, 20, ${s.bgAlpha / 255})`,
        }}
      >
        <div className="zh" style={{ fontSize: `${s.fontSize}px` }}>
          {cue?.zh ?? ""}
        </div>
        {s.showEn && cue?.en ? (
          <div className="en" style={{ fontSize: `${s.enSize}px` }}>
            {cue.en}
          </div>
        ) : null}
      </div>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<Overlay />);
