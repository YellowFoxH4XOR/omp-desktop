import type { InternImage } from '$lib/types';
export interface Screenshot extends InternImage { name: string }
export const MAX_ATTACHMENTS = 4;

/** Decode locally and downsize before any bytes cross IPC or reach a model. */
export async function screenshot(file: File): Promise<Screenshot> {
  if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) throw new Error('Use PNG, JPEG, or WebP screenshots.');
  if (file.size > 8 * 1024 * 1024) throw new Error('Screenshots must be under 8 MiB before resizing.');
  const bitmap = await createImageBitmap(file);
  try {
    if (!bitmap.width || !bitmap.height || bitmap.width > 20000 || bitmap.height > 20000) throw new Error('Screenshot dimensions are too large.');
    const scale = Math.min(1, 1600 / Math.max(bitmap.width, bitmap.height));
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(bitmap.width * scale));
    canvas.height = Math.max(1, Math.round(bitmap.height * scale));
    const context = canvas.getContext('2d');
    if (!context) throw new Error('Could not prepare screenshot.');
    context.fillStyle = '#fff';
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    for (const quality of [0.85, 0.65, 0.4]) {
      const data = canvas.toDataURL('image/jpeg', quality).split(',')[1];
      if (data && data.length <= 680 * 1024) return { name: file.name.slice(0, 160), mimeType: 'image/jpeg', data };
    }
    throw new Error('Screenshot is still too large. Crop it and try again.');
  } finally { bitmap.close(); }
}
