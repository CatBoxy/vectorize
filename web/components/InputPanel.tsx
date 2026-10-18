"use client";

import { useRef, useState } from "react";
import type { LoadedImage } from "@/lib/image";

interface Props {
  image: LoadedImage | null;
  onFile: (file: File) => void;
  onUrl: (url: string) => Promise<void>;
}

export default function InputPanel({ image, onFile, onUrl }: Props) {
  const fileRef = useRef<HTMLInputElement>(null);
  const [url, setUrl] = useState("");
  const [fetching, setFetching] = useState(false);

  const submit = async () => {
    const u = url.trim();
    if (!u) return;
    setFetching(true);
    try {
      await onUrl(u);
    } finally {
      setFetching(false);
    }
  };

  return (
    <section className="panel">
      <h2>Image</h2>
      <div className="row">
        <button type="button" onClick={() => fileRef.current?.click()}>
          Open file…
        </button>
        <span className="muted">or drop, or paste</span>
        <input
          ref={fileRef}
          type="file"
          accept="image/*"
          hidden
          onChange={(e) => {
            const f = e.target.files?.[0];
            if (f) onFile(f);
            e.target.value = "";
          }}
        />
      </div>
      <form
        className="row url"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <input type="url" placeholder="https://…/logo.png" value={url} onChange={(e) => setUrl(e.target.value)} />
        <button type="submit" disabled={fetching || !url.trim()}>
          {fetching ? "…" : "Fetch"}
        </button>
      </form>
      <p className="hint">The URL is fetched by this page, not by a server, so sites that block cross-origin requests (CORS) will fail: save the image and drop it instead.</p>
      {image && (
        <p className="muted">
          {image.name} · {image.originalWidth}×{image.originalHeight}
          {image.downscaled ? ` → ${image.width}×${image.height}` : ""}
        </p>
      )}
    </section>
  );
}
