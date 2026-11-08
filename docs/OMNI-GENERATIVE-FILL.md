# Omni Generative Fill

The ArtCraft API-key adapter supports `image.inpaint` for `nano_banana`, `nano_banana_2`
and `nano_banana_pro`. Listing a model does not verify an account or its credits. The
selected shared binding still runs through the normal plan, consent, start and wait flow.

## Request and result

The adapter validates the inline PNG source and mask, including matching dimensions and
bounded pixel count, before sending either. It converts the mask to opaque grayscale with
white meaning edit and black meaning keep. Each input is uploaded to
`/v1/omni_api/upload/image` as a private intermediate file with a fixed filename and a
fresh idempotency token. No local filename is uploaded.

The image-generation body contains the selected `model`, a fresh `idempotency_token`,
`image_media_tokens` in source, mask, then reference order, `image_batch_count: 1` and an
expanded `prompt`. The shared instruction asks the model to replace the white region, preserve
the black region and match the scene. An empty prompt asks it to continue the surroundings.

## Reference pictures

A fill may carry `reference` inputs. The adapter sets no count: how many a model takes is
Omni's to say, and it refuses a request with too many. That refusal comes after the uploads,
because Omni only judges the request when it is asked to generate. Each must be inline PNG data of at most 10 MB,
within the same pixel bounds as the source, and must decode in full. All of them are checked
before the first upload, so a refused reference means nothing was sent. They are uploaded after
the source and mask in request order, and the instruction names them as images 3 onwards and
tells the model to use them only as a guide, never to change the black region to match them.
A connection saved before references were mapped gains them, as an optional input, when the
store is read.

Known limit: if the service or network fails part-way through the uploads, the pictures
already uploaded stay on the account as private intermediate files. The Omni API has no call
to delete an upload (ArtCraft's own media-file delete needs a desktop session, not an API
key), so the adapter cannot remove them. Each reference adds one more upload that could be
left this way.

Omni receives two ordinary image references. There is no dedicated mask parameter or
pixel-preservation guarantee from this request. The host app must composite the result
through its own selection mask. Generation parameters are not mapped by this adapter. A
host may request several variations through separate jobs.

Fill results are retrieved as bounded image bytes from the documented HTTPS CDN, without
the API key, cookies or redirects, and returned as an inline payload. Loopback delivery
is allowed only when the configured API base is also loopback, for scripted tests.
Existing prompt-only image and video jobs continue to return media handles.

## Verification and limits

Scripted localhost tests cover the real adapter and the shared-client flow, including no
uploads before consent, private upload fields, selected-model preservation, input order,
one output per job, inline delivery and rejection of foreign hosts, redirects, non-image
responses and oversized results. Deliberately swapping input order, returning a handle
instead of inline bytes, and bypassing consent each caused a test failure.

On 2 November 2026, a local PhotoCraft build based on `storytold/photocraft#10903`
at `b2e4f6b5c`, with this adapter, completed a tester-triggered Nano Banana 2 fill on a
512 by 512 synthetic studio scene. The tester reported that the outputs did not blend
convincingly with the original. The app's completion status was observed, but the raw
request/output and final pixels were not captured. Saved-result integrity, outside-mask
pixel equality, undo/redo and seamless blending remain unverified. Nano Banana 1/Pro
have not been tested live. UI improvements and blending diagnostics are tracked in the
PhotoCraft review; this adapter change does not implement them.
