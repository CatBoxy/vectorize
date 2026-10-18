// Loading an image for tracing: decode in the browser, cap the long edge
// at 2048 px (every competitor does; the tracer is O(pixels) and the
// wasm memory is finite), and choose how the pixels travel to the worker.
// A PNG within the cap goes as its own bytes (the module decodes it); a
// JPEG goes through a canvas so the browser's EXIF orientation applies to
// what is traced exactly as to what is shown; anything else the browser
// can draw (WebP, GIF, BMP, AVIF) goes through the canvas too.

import type { TracePayload } from "./protocol";

export const MAX_EDGE = 2048;

export interface LoadedImage {
  name: string;
  /** Size as traced (after the cap). */
  width: number;
  height: number;
  originalWidth: number;
  originalHeight: number;
  downscaled: boolean;
  /** Object URL for display, same pixels as traced. */
  url: string;
  payload: TracePayload;
}

export async function loadImage(blob: Blob, name: string): Promise<LoadedImage> {
  let bmp: ImageBitmap;
  try {
    bmp = await createImageBitmap(blob);
  } catch {
    throw new Error("That file is not an image this browser can decode.");
  }
  const originalWidth = bmp.width;
  const originalHeight = bmp.height;
  if (originalWidth === 0 || originalHeight === 0) {
    bmp.close();
    throw new Error("The image is empty.");
  }
  const scale = Math.min(1, MAX_EDGE / Math.max(originalWidth, originalHeight));
  const width = Math.max(1, Math.round(originalWidth * scale));
  const height = Math.max(1, Math.round(originalHeight * scale));
  const downscaled = scale < 1;

  if (!downscaled && blob.type === "image/png") {
    bmp.close();
    const bytes = new Uint8Array(await blob.arrayBuffer());
    return {
      name,
      width,
      height,
      originalWidth,
      originalHeight,
      downscaled,
      url: URL.createObjectURL(blob),
      payload: { kind: "bytes", bytes },
    };
  }

  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) {
    bmp.close();
    throw new Error("No 2D canvas available.");
  }
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(bmp, 0, 0, width, height);
  bmp.close();
  const data = ctx.getImageData(0, 0, width, height);
  const rgba = new Uint8Array(data.data.buffer, data.data.byteOffset, data.data.byteLength);
  const url = downscaled
    ? await new Promise<string>((resolve, reject) =>
        canvas.toBlob((b) => (b ? resolve(URL.createObjectURL(b)) : reject(new Error("could not encode the downscaled image"))), "image/png"),
      )
    : URL.createObjectURL(blob);
  return {
    name,
    width,
    height,
    originalWidth,
    originalHeight,
    downscaled,
    url,
    payload: { kind: "rgba", width, height, rgba },
  };
}

/** Fetch an image URL from the page (CORS permitting). */
export async function fetchImage(url: string): Promise<{ blob: Blob; name: string }> {
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    throw new Error("That is not a valid URL.");
  }
  if (u.protocol !== "http:" && u.protocol !== "https:") throw new Error("Only http(s) URLs can be fetched.");
  let res: Response;
  try {
    res = await fetch(u.toString(), { mode: "cors", credentials: "omit" });
  } catch {
    throw new Error("Could not fetch it: the site does not allow cross-origin access (CORS). Save the image and drop it here instead.");
  }
  if (!res.ok) throw new Error(`The server answered ${res.status} ${res.statusText}.`);
  const blob = await res.blob();
  if (!blob.type.startsWith("image/")) throw new Error(`That URL is ${blob.type || "not an image"}, not an image.`);
  const name = decodeURIComponent(u.pathname.split("/").pop() || "image") || "image";
  return { blob, name };
}

export function baseName(name: string): string {
  const i = name.lastIndexOf(".");
  return (i > 0 ? name.slice(0, i) : name) || "trace";
}
