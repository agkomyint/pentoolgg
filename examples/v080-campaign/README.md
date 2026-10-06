# Orbital perspectives: compositing acceptance demo

This two-page, non-commercial technical study demonstrates linked photographs,
isolated groups, two reversible grades, a reusable feathered mask, a texture
clipped into live headline text, named gradient/shadow styles, and a perspective
screen mockup. It is a development example, not a released-format compatibility
claim. The composition was assembled with AI assistance; it is not NASA-approved
and implies no endorsement.

Generate it from the repository root into a **new** directory:

```sh
cargo run --locked --example v080_campaign -- target/orbital-study
pentool serve target/orbital-study/campaign.pen
pentool analyze target/orbital-study/campaign.pen --page poster --scope page --compare
pentool adjustment set target/orbital-study/campaign.pen global-grade --page poster --params '{"amount":-40}'
pentool undo target/orbital-study/campaign.pen
pentool redo target/orbital-study/campaign.pen
pentool linked collect target/orbital-study/campaign.pen --path target/orbital-portable
pentool export target/orbital-portable/document.pen target/orbital-portable/poster.png --page poster
```

The generator writes the editable document, two PNG previews and histogram JSON.
No source pixels are baked or overwritten. Use a `.pen` copy before grading to
compare structural/visual diffs. `tests/v080_campaign.rs` automates reversible
grading, history, compact serialization, preset dependency capture, deterministic
package creation, disconnecting originals, and identical decoded offline output
for both pages. Analytic blend/mask/effect goldens are covered by the other v080
tests. Matching results on all four hosted release targets are still required.

## Source provenance

Only the three small JPEG source files below are tracked; the generator does not
download anything. They are NASA-served 512-pixel renditions, downloaded October
6, 2026. Hashes identify the exact bytes used by this example, not the original
full-resolution photographs. The `.pen` image records repeat these hashes.

| File | SHA-256 | Source/credit |
|---|---|---|
| earth.jpg | `31b3ac8fc83fc84dfc624c9a406ab5ef11134e4e87412dffaeece5cb0dbb14aa` | [Blue Marble](https://www.nasa.gov/image-article/blue-marble-view-from-apollo-17/), NASA |
| sunrise.jpg | `a50e28b0250f5c2e551b3017a87608dd4db33db6b36d32bda7c40cd677092e62` | [Sunrise Begins](https://www.nasa.gov/image-article/sunrise-begins/), NASA/Matthew Dominick |
| horizon.jpg | `e63e7dfde437ee354e07f27ccf1555379f950754f01139648587f95cae5b7ab8` | [Orbital sunrise](https://www.nasa.gov/image-article/first-rays-of-an-orbital-sunrise-illuminate-earths-atmosphere-5/), NASA |

Downloads:

- `https://www.nasa.gov/wp-content/uploads/2023/03/as17-148-22727_lrg_0.jpg?w=512`
- `https://www.nasa.gov/wp-content/uploads/2024/08/iss071e487194orig.jpg?w=512`
- `https://www.nasa.gov/wp-content/uploads/2023/03/iss066e144602.jpg?w=512`

Use remains subject to [NASA's media guidelines](https://www.nasa.gov/nasa-brand-center/images-and-media/).
This informational example does not grant rights to NASA logos, identifiable
people, third-party material, or commercial endorsements. Do not treat the
modified composition as an unedited scientific image or a NASA statement.
