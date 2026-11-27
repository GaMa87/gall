// Shared image comparison logic used by both the disk adapter (`ImageCompare`) and the Git
// adapter (`GitImageDiff`): supported formats, size/pixel limits, unequal-dimension scaling,
// alpha detection, and pixel difference masking / statistics.

export const IMAGE_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "svg", "ico"]);
export const PIXEL_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp"]);

/** Maximum encoded image size shown in both disk and Git adapters (matches `gitimages.rs`). */
export const MAX_IMAGE_BYTES = 24 * 1024 * 1024;
/** Maximum decoded pixel count compared pixel-by-pixel (matches `diff.rs`). */
export const MAX_IMAGE_PIXELS = 60_000_000;
/** Channel delta above which a pixel counts as changed (matches `diff.rs`). */
export const PIXEL_DIFF_THRESHOLD = 10;

export interface ImageDims {
  width: number;
  height: number;
}

export interface ImagePairLayout {
  /** Shared bounding box (max width × max height) so unequal images share one pixel scale. */
  stageWidth: number;
  stageHeight: number;
  sizeMismatch: boolean;
  leftBox: { widthPct: number; heightPct: number };
  rightBox: { widthPct: number; heightPct: number };
  overlap: ImageDims;
}

export interface PixelDiffMask {
  compared: number;
  changed: number;
  extraPixels: number;
  maxDelta: number;
  width: number;
  height: number;
  mask: Uint8ClampedArray;
}

export interface DiffSummary {
  percentText: string;
  identical: boolean;
  sizeMismatch: boolean;
  overlapText: string | null;
}

export function imageExt(path: string): string {
  const name = path.split(/[\\/]/).pop() ?? path;
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? name.slice(dot + 1).toLowerCase() : "";
}

export function isImagePath(path: string): boolean {
  return IMAGE_EXT.has(imageExt(path));
}

export function canPixelDiff(left: string, right: string): boolean {
  return PIXEL_EXT.has(imageExt(left)) && PIXEL_EXT.has(imageExt(right));
}

/** Shared size limit error for encoded image files across disk and Git adapters. */
export function imageByteLimitMessage(sizeBytes: number | null | undefined, maxBytes = MAX_IMAGE_BYTES): string | null {
  if (sizeBytes == null || !Number.isFinite(sizeBytes) || sizeBytes <= maxBytes) return null;
  const mib = Math.round(maxBytes / (1024 * 1024));
  return `Images over ${mib} MiB are not shown`;
}

/** Shared pixel count limit for pixel-by-pixel difference rendering. */
export function imagePixelLimitMessage(dims: ImageDims | null | undefined, maxPixels = MAX_IMAGE_PIXELS): string | null {
  if (!dims || dims.width <= 0 || dims.height <= 0) return null;
  if (dims.width * dims.height <= maxPixels) return null;
  const mp = Math.round(maxPixels / 1_000_000);
  return `Images over ${mp} megapixels are too large to compare pixel by pixel`;
}

/**
 * Aligns two images onto a common coordinate scale (anchored at top-left, matching the
 * backend's `(0..minW, 0..minH)` pixel comparison) so unequal dimensions do not stretch
 * differently in Swipe or Onion skin modes.
 */
export function fitImagePair(left: ImageDims | null, right: ImageDims | null): ImagePairLayout | null {
  if (!left || !right || left.width <= 0 || left.height <= 0 || right.width <= 0 || right.height <= 0) return null;
  const stageWidth = Math.max(left.width, right.width);
  const stageHeight = Math.max(left.height, right.height);
  return {
    stageWidth,
    stageHeight,
    sizeMismatch: left.width !== right.width || left.height !== right.height,
    leftBox: {
      widthPct: (left.width / stageWidth) * 100,
      heightPct: (left.height / stageHeight) * 100,
    },
    rightBox: {
      widthPct: (right.width / stageWidth) * 100,
      heightPct: (right.height / stageHeight) * 100,
    },
    overlap: {
      width: Math.min(left.width, right.width),
      height: Math.min(left.height, right.height),
    },
  };
}

/** True when any pixel in an RGBA buffer has non-opaque alpha (< 255). */
export function hasAlphaTransparency(rgba: Uint8ClampedArray | Uint8Array): boolean {
  for (let i = 3; i < rgba.length; i += 4) {
    if (rgba[i] < 255) return true;
  }
  return false;
}

/**
 * Pure RGBA pixel-difference calculation matching `gall-core::fs::diff::image_diff`:
 * compares all 4 channels (including alpha) over the overlapping `(minW, minH)` region
 * and counts non-overlapping extra pixels when dimensions differ.
 */
export function computePixelDiffMask(
  leftRgba: Uint8ClampedArray | Uint8Array,
  leftDims: ImageDims,
  rightRgba: Uint8ClampedArray | Uint8Array,
  rightDims: ImageDims,
  threshold = PIXEL_DIFF_THRESHOLD,
): PixelDiffMask {
  const width = Math.min(leftDims.width, rightDims.width);
  const height = Math.min(leftDims.height, rightDims.height);
  const compared = width * height;
  const maxArea = Math.max(leftDims.width, rightDims.width) * Math.max(leftDims.height, rightDims.height);
  const extraPixels = Math.max(0, maxArea - compared);
  const mask = new Uint8ClampedArray(compared * 4);
  let changed = 0;
  let maxDelta = 0;

  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const li = (y * leftDims.width + x) * 4;
      const ri = (y * rightDims.width + x) * 4;
      const oi = (y * width + x) * 4;
      const dr = Math.abs(leftRgba[li] - rightRgba[ri]);
      const dg = Math.abs(leftRgba[li + 1] - rightRgba[ri + 1]);
      const db = Math.abs(leftRgba[li + 2] - rightRgba[ri + 2]);
      const da = Math.abs(leftRgba[li + 3] - rightRgba[ri + 3]);
      const d = Math.max(dr, dg, db, da);
      if (d > maxDelta) maxDelta = d;
      if (d > threshold) {
        changed++;
        mask[oi] = 235;
        mask[oi + 1] = 40;
        mask[oi + 2] = 70;
        mask[oi + 3] = 255;
      } else {
        const lum = Math.floor(0.299 * leftRgba[li] + 0.587 * leftRgba[li + 1] + 0.114 * leftRgba[li + 2]);
        const faded = 170 + Math.floor(lum / 3);
        mask[oi] = faded;
        mask[oi + 1] = faded;
        mask[oi + 2] = faded;
        mask[oi + 3] = 255;
      }
    }
  }
  return { compared, changed, extraPixels, maxDelta, width, height, mask };
}

export function formatDiffSummary(diff: {
  changed: number;
  compared: number;
  leftSize?: [number, number];
  rightSize?: [number, number];
}): DiffSummary {
  const ratio = diff.changed / Math.max(1, diff.compared);
  const percentText = (ratio * 100).toFixed(diff.changed > 0 && ratio < 0.001 ? 3 : 1);
  const sizeMismatch = !!(
    diff.leftSize && diff.rightSize
    && (diff.leftSize[0] !== diff.rightSize[0] || diff.leftSize[1] !== diff.rightSize[1])
  );
  const overlapText = sizeMismatch && diff.leftSize && diff.rightSize
    ? `${Math.min(diff.leftSize[0], diff.rightSize[0])}×${Math.min(diff.leftSize[1], diff.rightSize[1])} overlap`
    : null;
  return {
    percentText,
    identical: diff.changed === 0 && !sizeMismatch,
    sizeMismatch,
    overlapText,
  };
}
