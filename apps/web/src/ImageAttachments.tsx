import type { ImageAsset } from './api';

export function ImageAttachmentPicker({
  images,
  selected,
  enabled,
  busy,
  onChange,
}: {
  images: ImageAsset[];
  selected: string[];
  enabled: boolean;
  busy: boolean;
  onChange: (ids: string[]) => void;
}) {
  const bytes = images
    .filter((image) => selected.includes(image.id))
    .reduce((total, image) => total + image.bytes, 0);
  return (
    <fieldset className="chat-image-picker" aria-label="Chat images" disabled={busy}>
      <legend>Images for chat · {selected.length}/4</legend>
      <small>
        Attach up to four images (16 MiB total) for the selected model to inspect with this message.
        Later messages retain metadata; reattach an image to inspect it again.
      </small>
      {!enabled && <p>Select a model profile with vision enabled to send images.</p>}
      {!images.length && <p>Upload an image in Assets, or generate one, then attach it here.</p>}
      {images.map((image) => {
        const checked = selected.includes(image.id);
        return (
          <label key={image.id} className="request-permission">
            <input
              type="checkbox"
              checked={checked}
              aria-label={`Attach ${image.name} to chat`}
              disabled={
                !checked &&
                (!enabled || selected.length >= 4 || bytes + image.bytes > 16 * 1024 * 1024)
              }
              onChange={() =>
                onChange(
                  checked ? selected.filter((id) => id !== image.id) : [...selected, image.id],
                )
              }
            />
            {image.name}
          </label>
        );
      })}
      {!!selected.length && (
        <p>Selected images will be sent to the model’s configured connection.</p>
      )}
    </fieldset>
  );
}

export function ImageAttachmentView({ images }: { images: ImageAsset[] }) {
  if (!images.length) return null;
  return (
    <div className="chat-image-attachments" aria-label="Attached chat images">
      {images.map((image) => (
        <a key={image.id} href={`/api/image-assets/${image.id}/content`}>
          <img
            src={`/api/image-assets/${image.id}/content`}
            alt={image.name}
            loading="lazy"
            width="96"
            height="96"
          />
          <span>{image.name}</span>
        </a>
      ))}
    </div>
  );
}
