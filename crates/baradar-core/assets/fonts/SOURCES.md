# Bundled fonts

Every font here is licensed under the **SIL Open Font License 1.1** or the **Apache License 2.0**;
the licence text of each family sits next to its files (`OFL-<Family>.txt` or
`Apache-<Family>.txt`, copied unchanged from upstream). `web/src/render/fonts.ts` loads them with
the `FontFace` API under private family names ("ptouch Oswald"…), and
`web/src/render/font-catalog.ts` lists them.

There are two sets:

- **Core** (Fira Sans, Archivo Narrow, JetBrains Mono, Atkinson Hyperlegible): precached by the
  service worker with the app, so they work offline from the first visit.
- **Library** (21 families): loaded the first time a label or the font picker uses them, then kept
  by the service worker (`CacheFirst`, cache `ptouch-fonts`, `pwa.config.ts`), so a font used once
  also works offline. No font is ever requested from a third-party host.

**Rule for library files: a file never changes content under the same name.** The runtime cache
has no revisions, so new bytes (a newer upstream, other weights, another subset) need a new file
name. Library file names never start with a core family's prefix (`FiraSans`, `ArchivoNarrow`,
`JetBrainsMono`, `AtkinsonHyperlegible`): the precache glob selects the core files by prefix.

## Subsetting

Every file was downloaded from the official upstream source below. All of them except the two
families with a Reserved Font Name (Fira Sans and Quicksand, shipped unmodified: see their
sections) were then subset with `pyftsubset` (fontTools 4.60.2, brotli 1.2.0) to woff2:

```sh
pyftsubset <upstream>.woff2 --flavor=woff2 --name-IDs='*' --name-languages='*' \
  --layout-features+=tnum,zero,frac,sups,subs,case --unicodes="$U" --output-file=<file>.woff2
```

`$U` is `U+0020-007E,U+00A0-017F,U+0192,U+0218-021B,U+02C6-02DD,U+0300-0308,U+030A-030C,U+0327,U+0370-03FF,U+1E9E,U+2000-206F,U+2070-209F,U+20A0-20CF,U+2100-214F,U+2150-218F,U+2190-21FF,U+2200-22FF,U+2300-23FF,U+2460-24FF,U+2500-257F,U+25A0-25FF,U+2600-26FF,U+2713-2717,U+FB01-FB02,U+FEFF,U+FFFD`.

That keeps Latin (incl. Latin-1 and Latin Extended-A), basic Greek (µ, Ω, Δ…), punctuation,
currency, letterlike symbols (№, ℃, Ω), number forms, arrows, maths (±, ≤, ×, √, ∞), technical
symbols (⌀) and geometric shapes, as far as each font covers them. Name tables (copyright and
licence) are kept in full. Italics are not bundled: the browser synthesises an oblique.

Library fonts that upstream ships only as a variable font were first instanced to static weights
with `fonttools varLib.instancer <file>.ttf wght=<w> [<axis>=<v>] --static --update-name-table
--no-recalc-timestamp`, then subset with the command above. Pixel fonts cover fewer characters;
the same command keeps whatever they have.

## Core fonts

**Reserved Font Name.** Fira Sans's `OFL.txt` has no Reserved Font Name clause, but the
copyright record inside every Fira Sans font (nameID 0) reads *"… bBox Type GmbH and Carrois
Corporate GbR, with Reserved Font Name "Fira"*". Subsetting is a modification under the OFL, so a
subset could not keep the name "Fira Sans". The five Fira Sans files are therefore the
**unmodified upstream WOFF2 files**, byte for byte (upstream SHA-256 = bundled SHA-256, all 2,881
glyphs, about 160 KB each), not subsets. Archivo Narrow, JetBrains Mono and Atkinson Hyperlegible
declare no Reserved Font Name in their `OFL.txt` or their name tables, so they are subset as
described above. `tests/unit/render/font-library.test.ts` checks the RFN rule for every bundled
family, core included.

| File | Family | Weight | Upstream (version) | Upstream SHA-256 | Bundled SHA-256 |
|---|---|---|---|---|---|
| `FiraSans-Regular.woff2` | Fira Sans | 400 | [bBoxType/FiraSans](https://github.com/bBoxType/FiraSans) 4.301, commit `f54eeb3124c63fe9b5bcd36d09d1cd46788cd15e`, `Fira_Sans_4_3/Fonts/Fira_Sans_WEB_4301/Normal/Roman/FiraSans-Regular.woff2` | `51000d3cc8a601427bdb88625275e0eefc00570f4f2ab7a926fa336abee7098f` | `51000d3cc8a601427bdb88625275e0eefc00570f4f2ab7a926fa336abee7098f` |
| `FiraSans-Medium.woff2` | Fira Sans | 500 | same, `…/FiraSans-Medium.woff2` | `14d421d4fb35d56b40e958cc9b64eb1528d1822bb75251623955c873a9a175d3` | `14d421d4fb35d56b40e958cc9b64eb1528d1822bb75251623955c873a9a175d3` |
| `FiraSans-SemiBold.woff2` | Fira Sans | 600 | same, `…/FiraSans-SemiBold.woff2` | `01ca0c4a1f02dd4324721cf8766bcbb2edca4dee0fb8884a66e58e231ed62d55` | `01ca0c4a1f02dd4324721cf8766bcbb2edca4dee0fb8884a66e58e231ed62d55` |
| `FiraSans-Bold.woff2` | Fira Sans | 700 | same, `…/FiraSans-Bold.woff2` | `7dff4e2351ca54fae96180e79ab1d1fdeb74ebb6e6cba53a7d9d7b4941600ada` | `7dff4e2351ca54fae96180e79ab1d1fdeb74ebb6e6cba53a7d9d7b4941600ada` |
| `FiraSans-ExtraBold.woff2` | Fira Sans | 800 | same, `…/FiraSans-ExtraBold.woff2` | `133482a0c7aa051cd89a9b6bf5ee990bad1bf1a4b37918b96a0d626fdb529bec` | `133482a0c7aa051cd89a9b6bf5ee990bad1bf1a4b37918b96a0d626fdb529bec` |
| `ArchivoNarrow-Regular.woff2` | Archivo Narrow | 400 | [Omnibus-Type/ArchivoNarrow](https://github.com/Omnibus-Type/ArchivoNarrow) commit `9793ec77b6682a26bc7a6ed523ca65cc3cb90aec`, `fonts/webfonts/ArchivoNarrow-Regular.woff2` | `5b34e15998e0a4d6b6bf3377ebed3b9f3189f8a5511e3afbc2a7e0cfe1d03fdf` | `a4f9c875991ca6f715824dde6e26670dd8c923164e47a0a31524ee9180027e57` |
| `ArchivoNarrow-Medium.woff2` | Archivo Narrow | 500 | same, `fonts/webfonts/ArchivoNarrow-Medium.woff2` | `523fa4e3a2bd3b16f70022c75a3106410ec02eb5bf19ef720350d3c301e24581` | `08bdb13cef726fe839946079b6e2050be23ecfa1dda425e9983a45fcde0e7f49` |
| `ArchivoNarrow-SemiBold.woff2` | Archivo Narrow | 600 | same, `fonts/webfonts/ArchivoNarrow-SemiBold.woff2` | `ca6a15c936cc8c4cbdaf6a2aa0c4ff467b56625ed67e2cf4caecd41861c637da` | `31127f34364dce500c1cc55ba6de53aaea09364bff0b9dcb1e4695391d061262` |
| `ArchivoNarrow-Bold.woff2` | Archivo Narrow | 700 | same, `fonts/webfonts/ArchivoNarrow-Bold.woff2` | `c7143f25df2786f13467f13afa0bde2cb1a4844bd1606ae08127b9f76b524743` | `383c773dfe5b02744f69958c598ed2be808c40602c0025bd036f59b3c47f9d84` |
| `JetBrainsMono-Regular.woff2` | JetBrains Mono | 400 | [JetBrains/JetBrainsMono](https://github.com/JetBrains/JetBrainsMono) release v2.304, `JetBrainsMono-2.304.zip` (SHA-256 `6f6376c6ed2960ea8a963cd7387ec9d76e3f629125bc33d1fdcd7eb7012f7bbf`), `fonts/webfonts/JetBrainsMono-Regular.woff2` | `a9cb1cd82332b23a47e3a1239d25d13c86d16c4220695e34b243effa999f45f2` | `3183eddab3524a5802bf33933e56cada800c570db2aacbe5b1cdead4e0fad6db` |
| `JetBrainsMono-Bold.woff2` | JetBrains Mono | 700 | same zip, `fonts/webfonts/JetBrainsMono-Bold.woff2` | `c503cc5ec5f8b2c7666b7ecda1adf44bd45f2e6579b2eba0fc292150416588a2` | `87016d2525f1edec69345e5a8cf47fbcd7123ba57231352b5342f82b126f9573` |
| `AtkinsonHyperlegible-Regular.woff2` | Atkinson Hyperlegible | 400 | [googlefonts/atkinson-hyperlegible](https://github.com/googlefonts/atkinson-hyperlegible) (Braille Institute) commit `1cb311624b2ddf88e9e37873999d165a8cd28b46`, `fonts/webfonts/AtkinsonHyperlegible-Regular.woff2` | `2df4ba17804bc7a36f123127966075d8427bff2df58d0d76820c1130bb1a4150` | `077f49f1d8e26821a1f67a071af09360fa474303c76c205c672575d81f05202f` |
| `AtkinsonHyperlegible-Bold.woff2` | Atkinson Hyperlegible | 700 | same, `fonts/webfonts/AtkinsonHyperlegible-Bold.woff2` | `da8fce41a04f8498fbf79076f92d304b12e70c76f71b143c5dcfb6536c93c075` | `dd36c3052f098a4dd010c7ee64e0574f07308fbbd9a2443af1ed46395335d4fa` |

### Licence files

| File | Upstream | SHA-256 |
|---|---|---|
| `OFL-FiraSans.txt` | bBoxType/FiraSans `OFL.txt` (same commit) | `5c29650250730778eccb5475b112d32a8e0c9dd1860d9509693329652bf8e9eb` |
| `OFL-ArchivoNarrow.txt` | Omnibus-Type/ArchivoNarrow `OFL.txt` (same commit) | `b2087ef3fb91248e346600d19021249152a9688efea72905aa1996bb16a4c8de` |
| `OFL-JetBrainsMono.txt` | JetBrainsMono-2.304.zip `OFL.txt` | `30f0c136e3c88e422d0791acd97238870f9054a9729bc34cf2ff0d4ed8cac4ad` |
| `OFL-AtkinsonHyperlegible.txt` | googlefonts/atkinson-hyperlegible `OFL.txt` (same commit) | `f32d22b3908fcad2c86a74000614ec22e6a7f66ea7e867e616026a27aebdc143` |

Core total: 13 woff2 files, about 1.06 MB (of which the five unmodified Fira Sans files are 818 KB).

## Library fonts

**Upstream:** [google/fonts](https://github.com/google/fonts) at commit
`f2bd09badbc763d8757951d52deec29da27e85fb` (2026-09-18), the repository Google Fonts publishes from. Files were fetched from
`https://raw.githubusercontent.com/google/fonts/f2bd09badbc763d8757951d52deec29da27e85fb/<path>` and each one's git blob hash was
checked against that commit's tree before use. The designers' own repositories are named in each
licence's copyright line. The build script (instancer + `pyftsubset` above) is reproducible: two
runs give byte-identical files.

### Licences and Reserved Font Names

Every `OFL.txt` and every font's name table (copyright, nameID 0; licence, nameID 13) was read for
"Reserved Font Name". Subsetting and instancing are modifications under the OFL (OFL-FAQ 1.1-update7),
so a family with a Reserved Font Name (RFN) may only be shipped unmodified or renamed.

| Family | Licence | Copyright (upstream) | RFN finding | Shipped as |
|---|---|---|---|---|
| Oswald | OFL-1.1 | 2016 The Oswald Project Authors | none | instances 400, 700; subset |
| Bebas Neue | OFL-1.1 | 2010 Dharma Type / 2019 The Bebas Neue Project Authors | none | subset |
| Barlow Condensed | OFL-1.1 | 2017 The Barlow Project Authors | none | Medium, Bold; subset |
| Anton | OFL-1.1 | 2020 The Anton Project Authors | none | subset |
| Barlow | OFL-1.1 | 2017 The Barlow Project Authors | none | Regular, SemiBold; subset |
| B612 | OFL-1.1 (upstream is dual EPL-1.0 / OFL-1.1; shipped under the OFL) | 2012 The B612 Project Authors | none | Regular, Bold; subset |
| Nunito | OFL-1.1 | 2014 The Nunito Project Authors | none | instances 400, 700; subset |
| Fredoka | OFL-1.1 | 2016 The Fredoka Project Authors | none | instances 400, 600 at `wdth=100`; subset |
| Quicksand | OFL-1.1 | 2011/2019 The Quicksand Project Authors | **RFN "Quicksand"** (OFL.txt and nameID 0) | **unmodified**, see below |
| Caveat | OFL-1.1 | 2014 The Caveat Project Authors | none | instances 400, 700; subset |
| Permanent Marker | Apache-2.0 | 2010 Font Diner, Inc. | n/a (Apache-2.0 has no RFN; "All rights reserved" in nameID 0 is the usual copyright line) | subset |
| Pacifico | OFL-1.1 | 2018 The Pacifico Project Authors | none | subset |
| Big Shoulders Stencil | OFL-1.1 | 2019 The Big Shoulders Project Authors | none | instances 400, 700 at `opsz=72`; subset |
| Saira Stencil One | OFL-1.1 | 2019 The Saira Stencil Project Authors | none | subset |
| Special Elite | Apache-2.0 | 2010 Brian J. Bonislawsky DBA Astigmatic (AOETI) | n/a | subset |
| Courier Prime | OFL-1.1 | 2015 The Courier Prime Project Authors | none | Regular, Bold; subset |
| Roboto Slab | Apache-2.0 | 2018 The Roboto Slab Project Authors | n/a | instances 400, 700; subset |
| Silkscreen | OFL-1.1 | 2001 The Silkscreen Project Authors | none | Regular, Bold; subset |
| VT323 | OFL-1.1 | 2011 The VT323 Project Authors | none | subset |
| Pixelify Sans | OFL-1.1 | 2021 The Pixelify Sans Project Authors | none | instances 400, 700; subset |
| Lexend | OFL-1.1 | 2018/2019 The Lexend Project Authors | RFN **"RevReading Lexend"** (OFL.txt only) | instances 400, 700; subset |

- **Quicksand** reserves its name, so it is shipped **unmodified**: the upstream variable font
  (`Quicksand[wght].ttf`, all glyphs, weights 300–700) re-wrapped as WOFF2 by
  `fontTools.ttLib.woff2.compress(…, transform_tables=set())`, i.e. without any WOFF2 table
  transform and without WOFF metadata. OFL-FAQ 2.2 allows a WOFF/WOFF2 version under the original
  name when "the original font data remains unchanged except for WOFF compression". Checked
  table by table after decoding: all 19 tables are byte-identical to the TTF except `head`, where
  only what the WOFF2 format itself prescribes differs (flags bit 11, "font data is lossless
  after compression", and the recomputed `checkSumAdjustment`), and the 8-byte placeholder `DSIG`
  (no signatures) is dropped, as WOFF2 encoders do because re-encoding would invalidate a
  signature. The one file serves both picker weights, 500 and 700: the browser sets the `wght`
  axis from the requested weight. Upstream ships no woff2 of its own
  (andrew-paglinawan/QuicksandFamily has TTF only).
- **Lexend** reserves only the name "RevReading Lexend". The modified (subset, instanced) files
  are named "Lexend" in their name tables and in the picker, which does not use the reserved
  name, so subsetting is allowed.
- **Apache-2.0 fonts** (Permanent Marker, Special Elite, Roboto Slab): the files here are
  **modified** versions (instanced where noted, subset, converted to WOFF2 by the ptouch
  project, Apache-2.0 §4(b)); the copyright notices in their name tables are kept. Upstream has
  no NOTICE file.
- **D-DIN** was not added: it has no official, versioned upstream repository to pin
  (docs/FONTS-AND-SIZE-PLAN.md §3.1). Barlow and B612 cover that look.

### Files

| File | Family | Weight | Upstream path (google/fonts @ `f2bd09b`) | Processing | Upstream SHA-256 | Bundled SHA-256 | Size |
|---|---|---|---|---|---|---|---|
| `Oswald-Regular.woff2` | Oswald | 400 | `ofl/oswald/Oswald[wght].ttf` | instance `wght=400`, subset | `5b38c246e255a12f5712d640d56bcced0472466fc68983d2d0410ec0457c2817` | `9242c2d5d814242b3dcd5f10111f6dfc8031169f9d3076ef63d5947943d1e5d8` | 16 KB |
| `Oswald-Bold.woff2` | Oswald | 700 | `ofl/oswald/Oswald[wght].ttf` | instance `wght=700`, subset | `5b38c246e255a12f5712d640d56bcced0472466fc68983d2d0410ec0457c2817` | `e5971375fef1b13c3cd71e24d28c83dcbc56c041dd3621fdea8fb4609769508c` | 17 KB |
| `BebasNeue-Regular.woff2` | Bebas Neue | 400 | `ofl/bebasneue/BebasNeue-Regular.ttf` | subset | `08e4623805102d819f58601e46e345648846075e363b2ceb23313c2d1c83ec73` | `a650267bb293ad026c72363da78f1417a8696ff66c05f6ac56b7fe73cae40e28` | 17 KB |
| `BarlowCondensed-Medium.woff2` | Barlow Condensed | 500 | `ofl/barlowcondensed/BarlowCondensed-Medium.ttf` | subset | `262bd143292ce479ee0cd09a42b47ab173fca8e9c6eb5ed0b5c8a845bc371d17` | `9476104687cad57312c2db7d31e9e835ad852f963e908c8ec2e8590478b5988f` | 26 KB |
| `BarlowCondensed-Bold.woff2` | Barlow Condensed | 700 | `ofl/barlowcondensed/BarlowCondensed-Bold.ttf` | subset | `e476562ec9c1e16cf16475895b511f08c804f438cc9a9f80a44ea50a0eeb5b65` | `44af4eb92ebf4c5be15dfe6dbb49a7bb60be3877e72495ffb525c38ffbe3445c` | 27 KB |
| `Anton-Regular.woff2` | Anton | 400 | `ofl/anton/Anton-Regular.ttf` | subset | `a4ba3a92350ebb031da0cb47630ac49eb265082ca1bc0450442f4a83ab947cab` | `7a3bd15e74a4e8f4088a66ad057fdb7fde475934eea911b383e50afc8f7b5c6a` | 26 KB |
| `Barlow-Regular.woff2` | Barlow | 400 | `ofl/barlow/Barlow-Regular.ttf` | subset | `95aa02c7c43096e0dd44d787ba6216864a67157e402adab59b35572e0c1577ea` | `c16b2b22b4781def7bd944cb9d8350dccc2407b066401bd861ffa99d6f4d2ca2` | 26 KB |
| `Barlow-SemiBold.woff2` | Barlow | 600 | `ofl/barlow/Barlow-SemiBold.ttf` | subset | `86577cb32f8abe3673db53ca0f4221e6856751a4f6730c867e00f720f8bb1fc5` | `79b5c68bc3ec174580d01c176d88c4670148456c690c354d9b1b1d8e4d4b2c99` | 27 KB |
| `B612-Regular.woff2` | B612 | 400 | `ofl/b612/B612-Regular.ttf` | subset | `139dce659100a83bf95b48474696e448bee95631ef84fd3d0437ced2bf33cf73` | `660da8f85b7b94b8a5568ad929315530e477cb11f7427ec79ae8ebde8e69eda1` | 36 KB |
| `B612-Bold.woff2` | B612 | 700 | `ofl/b612/B612-Bold.ttf` | subset | `91749541ac7b2c328b58832b7e2c4df809d7e2ba38d62a3a5aa3f8e38b271814` | `3dfa80eeebd2521a957227a18a57947a3ebdea2caf30ddec71f593417a50d356` | 25 KB |
| `Nunito-Regular.woff2` | Nunito | 400 | `ofl/nunito/Nunito[wght].ttf` | instance `wght=400`, subset | `bb55a5ca5c2042335b3991af27c4d0705d0ef41cac6164ac737fd8f2a1e85207` | `0cc5c3ecd80ad7f656f8fc727d91145e69366bc62bc406ce5d8c92d3c799ce6b` | 22 KB |
| `Nunito-Bold.woff2` | Nunito | 700 | `ofl/nunito/Nunito[wght].ttf` | instance `wght=700`, subset | `bb55a5ca5c2042335b3991af27c4d0705d0ef41cac6164ac737fd8f2a1e85207` | `5561202df4ab7b769d1162388925a7be9192db180db8f9151bdad510204b5b1e` | 22 KB |
| `Fredoka-Regular.woff2` | Fredoka | 400 | `ofl/fredoka/Fredoka[wdth,wght].ttf` | instance `wght=400 wdth=100`, subset | `2ba02e68b152868aef9ba28e24b3648c7d457fe6f25c761f2c2c53fb61a73fc8` | `736b7467f49de9e0920503c890a4907ab82d6c746896ce8f9e831dab08f8647c` | 16 KB |
| `Fredoka-SemiBold.woff2` | Fredoka | 600 | `ofl/fredoka/Fredoka[wdth,wght].ttf` | instance `wght=600 wdth=100`, subset | `2ba02e68b152868aef9ba28e24b3648c7d457fe6f25c761f2c2c53fb61a73fc8` | `8228b152d970da6b59ee350b71e00e7d4b883c6f55f562305747f197d5bb96cc` | 16 KB |
| `Quicksand-Variable.woff2` | Quicksand | 500, 700 | `ofl/quicksand/Quicksand[wght].ttf` | WOFF2 only (unmodified, see RFN) | `39c9b64223561f56aaff6062a6f04063c4fc86809ad6768722c06614d977e1cc` | `3ae17adca3eeb083d4953c00ce5847df7d1359aadd5c75995bae7698a2ae7742` | 55 KB |
| `Caveat-Regular.woff2` | Caveat | 400 | `ofl/caveat/Caveat[wght].ttf` | instance `wght=400`, subset | `0bdb6b660482d31531b3945849fba5916b3ef8695da7024a9e6b9ee3c4157988` | `89c105f1173a98f6c5efed6f9eb5a00070a819f8131b87b2d88cebcd82784f12` | 54 KB |
| `Caveat-Bold.woff2` | Caveat | 700 | `ofl/caveat/Caveat[wght].ttf` | instance `wght=700`, subset | `0bdb6b660482d31531b3945849fba5916b3ef8695da7024a9e6b9ee3c4157988` | `b107b8beb55231736dbe3a9fb68822f6864fbed5ee0c4aa4d0e5a7e610c5069f` | 57 KB |
| `PermanentMarker-Regular.woff2` | Permanent Marker | 400 | `apache/permanentmarker/PermanentMarker-Regular.ttf` | subset | `28f82c8a7943cb8e9d599f8554da1d4fc75dbcf69b9885ad6c0611d20c6946c5` | `cb42def9253e4e3c08dfa42969523b5a8ada8fc3b2e55f806e68f08b03832396` | 29 KB |
| `Pacifico-Regular.woff2` | Pacifico | 400 | `ofl/pacifico/Pacifico-Regular.ttf` | subset | `5b6c0d5334a7bf77dea52b975c5a0c408878c0f7115ed5b6fb151f634b7bf701` | `8ea948e743b4103604b4e44b1dc93e0fbfa3847794474cfc5c85d89cd6c638c2` | 48 KB |
| `BigShouldersStencil-Regular.woff2` | Big Shoulders Stencil | 400 | `ofl/bigshouldersstencil/BigShouldersStencil[opsz,wght].ttf` | instance `wght=400 opsz=72`, subset | `758ec880296a8bdaf736a31fc57e90fa16673d8c357efc3c36bdb6582d625f0a` | `f9ce5e47337448a688c234741d8fc55d90a37f3c41e8a4e7b5eb33e52d7dc670` | 19 KB |
| `BigShouldersStencil-Bold.woff2` | Big Shoulders Stencil | 700 | `ofl/bigshouldersstencil/BigShouldersStencil[opsz,wght].ttf` | instance `wght=700 opsz=72`, subset | `758ec880296a8bdaf736a31fc57e90fa16673d8c357efc3c36bdb6582d625f0a` | `0314db8b406bcdcb1a5d88a8d447dbb93b28a9f9cc674b0201d36a31f396e2e1` | 20 KB |
| `SairaStencilOne-Regular.woff2` | Saira Stencil One | 400 | `ofl/sairastencilone/SairaStencilOne-Regular.ttf` | subset | `781496fdaf8e04cf6741b31025f6b4ba84f66021b097a8e0d85cbea2180cf223` | `fba9fee50dc920355d9a0940ca61f99e41141479fdba530fde24e6f2029f4907` | 27 KB |
| `SpecialElite-Regular.woff2` | Special Elite | 400 | `apache/specialelite/SpecialElite-Regular.ttf` | subset | `a776fcb4ceb8bdf03e2967688ebdad42680de5b91a7e62c17e718ae212d14bc4` | `9093614ab0f7b4d7e0846d808f95ec5b2edc4906105f016e362228c21e59dd75` | 58 KB |
| `CourierPrime-Regular.woff2` | Courier Prime | 400 | `ofl/courierprime/CourierPrime-Regular.ttf` | subset | `72f793376f8e2841656bf21d77a5de010f2929bd6956a22ee848ad0c7eb978af` | `9f57b7b9823a6c974c2c3a8e5709c681bf7f580e8042a2cf219a8b594ae59084` | 24 KB |
| `CourierPrime-Bold.woff2` | Courier Prime | 700 | `ofl/courierprime/CourierPrime-Bold.ttf` | subset | `ff1f38786c849d1c41fa8e447960abdb2bd75fdfb0cfcdeb524fad65a5af3638` | `25ae7a18d312b8865e4623e3f77147c3fa7b0295b06618fd54aabf2182fd319e` | 25 KB |
| `RobotoSlab-Regular.woff2` | Roboto Slab | 400 | `apache/robotoslab/RobotoSlab[wght].ttf` | instance `wght=400`, subset | `786ae192477447d33c6672c3055fba7cbfe45184c9a79e77a14f15716ca05b16` | `f6d90c4c729d0c197e8c50f97cf1b2a5f724003a279f0b2b444ae306181487f8` | 20 KB |
| `RobotoSlab-Bold.woff2` | Roboto Slab | 700 | `apache/robotoslab/RobotoSlab[wght].ttf` | instance `wght=700`, subset | `786ae192477447d33c6672c3055fba7cbfe45184c9a79e77a14f15716ca05b16` | `3dadbdba643248b5a091546967a47b165b3627685e99f9955422aa235c4466f5` | 21 KB |
| `Silkscreen-Regular.woff2` | Silkscreen | 400 | `ofl/silkscreen/Silkscreen-Regular.ttf` | subset | `c845473330b94c2079ce9af01c51ac8ba2d99c24f4d14c039843bbb8e642ebd8` | `3b1b298bf734c16674178dae42e275db6ad9cd1f55bbe9640e845815c0e0929f` | 8 KB |
| `Silkscreen-Bold.woff2` | Silkscreen | 700 | `ofl/silkscreen/Silkscreen-Bold.ttf` | subset | `768476aa712d4f5c3e18d3bce80f980a8bd3f72b7094d22ec5e768df3acfed61` | `a5dd2eec3d6209176cc0ca7553aa5e23d87d9c9bd6f9b1932bff463d8a926e8c` | 7 KB |
| `VT323-Regular.woff2` | VT323 | 400 | `ofl/vt323/VT323-Regular.ttf` | subset | `cf4de751ada78ceac033dbe16a687742939995b77bc2a052ae17a4957958594d` | `6a1b303a6b3d360baa8c5f745eb318c3f00f49db5af89e5538117df678f81e23` | 23 KB |
| `PixelifySans-Regular.woff2` | Pixelify Sans | 400 | `ofl/pixelifysans/PixelifySans[wght].ttf` | instance `wght=400`, subset | `9ba86cd010a4de309d263ceff8e8044092c9db7efda869620cb9ff1c4389e8a5` | `b470d98a2e5b572d63c1db23aacfd2e58e497c2ede149afb838391c853bc815b` | 10 KB |
| `PixelifySans-Bold.woff2` | Pixelify Sans | 700 | `ofl/pixelifysans/PixelifySans[wght].ttf` | instance `wght=700`, subset | `9ba86cd010a4de309d263ceff8e8044092c9db7efda869620cb9ff1c4389e8a5` | `a2f8e7fe0427d1d397683fee1a48ce01f0ef7d47424954e92c035047fe03d1bb` | 11 KB |
| `Lexend-Regular.woff2` | Lexend | 400 | `ofl/lexend/Lexend[wght].ttf` | instance `wght=400`, subset | `3add53e641fbc81da64da4bb254285e2831b52b029527bc0714e2b9610832ee6` | `9c0b87044d16a8bdef11fc14b546ef23f0cd79987356123a280ae93a302cc054` | 19 KB |
| `Lexend-Bold.woff2` | Lexend | 700 | `ofl/lexend/Lexend[wght].ttf` | instance `wght=700`, subset | `3add53e641fbc81da64da4bb254285e2831b52b029527bc0714e2b9610832ee6` | `c7a379c6fc3b3864b8ba99da1e41b4fb711e4e674a329ade6fe21d87c172656f` | 19 KB |

| File | Upstream (same commit) | SHA-256 |
|---|---|---|
| `OFL-Oswald.txt` | `ofl/oswald/OFL.txt` | `0fd731a904b729a4e02eaf5e8ebd06783edd9abe400e8882760160230675b652` |
| `OFL-BebasNeue.txt` | `ofl/bebasneue/OFL.txt` | `72082f6cb4d04be2ecf7cc7d9e1e7d73787f0af8a5a278a47cade70c16b78341` |
| `OFL-BarlowCondensed.txt` | `ofl/barlowcondensed/OFL.txt` | `186d750eb496a4c17a76385f82be6aea2ac1cf2de074a811d63786cf374ea73f` |
| `OFL-Anton.txt` | `ofl/anton/OFL.txt` | `ee67e6ee22790b7929f1a3769ca2801d565c64b5a9096942c1adf5596de9c9e4` |
| `OFL-Barlow.txt` | `ofl/barlow/OFL.txt` | `186d750eb496a4c17a76385f82be6aea2ac1cf2de074a811d63786cf374ea73f` |
| `OFL-B612.txt` | `ofl/b612/OFL.txt` | `a815f65bc72d90494b01842d3171f7cb0f9f935e023d9cc260904d221ef1064a` |
| `OFL-Nunito.txt` | `ofl/nunito/OFL.txt` | `580df76c95a1ec5ab878ceb25bb3d85c6a076804e9c970c8c6972aea775fdf65` |
| `OFL-Fredoka.txt` | `ofl/fredoka/OFL.txt` | `5c9e7eee5c6b25f4b05b8d53b2e470ea4962f9ced742d044a98f7d95d1375bab` |
| `OFL-Quicksand.txt` | `ofl/quicksand/OFL.txt` | `14d28541780d90f6577bb7abcc6f005bf6969614294e75c82abb8758a696ad46` |
| `OFL-Caveat.txt` | `ofl/caveat/OFL.txt` | `1f9d81d094273d82f3898a1ee8b598a717d050ecbf5ff7bede105b704880157b` |
| `Apache-PermanentMarker.txt` | `apache/permanentmarker/LICENSE.txt` | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |
| `OFL-Pacifico.txt` | `ofl/pacifico/OFL.txt` | `a47e5daeda73568969395c656823102678f2eefb0d7d7ecb47aac4cc17e42204` |
| `OFL-BigShouldersStencil.txt` | `ofl/bigshouldersstencil/OFL.txt` | `fbc746aabf0eb1847dfd92e2efc4596d79fa897d60b8e64062a22f585508fb3f` |
| `OFL-SairaStencilOne.txt` | `ofl/sairastencilone/OFL.txt` | `fc7c16a0d286a351a7de245a0bd661275e9d393de8955c7fa37ef3c8b85b7e3e` |
| `Apache-SpecialElite.txt` | `apache/specialelite/LICENSE.txt` | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |
| `OFL-CourierPrime.txt` | `ofl/courierprime/OFL.txt` | `9a755af092b494944c99f471be6fddd19b006a448fefdc4717e4ee0aa09a97b0` |
| `Apache-RobotoSlab.txt` | `apache/robotoslab/LICENSE.txt` | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |
| `OFL-Silkscreen.txt` | `ofl/silkscreen/OFL.txt` | `86c5e9c9382cdcc5948704fdfe60f2aa164a719746931219a42736ecd9cefbd3` |
| `OFL-VT323.txt` | `ofl/vt323/OFL.txt` | `27d9af34210253e7ca1251fbace86c6f65b40031d6ce1a75493a1b2093631298` |
| `OFL-PixelifySans.txt` | `ofl/pixelifysans/OFL.txt` | `b66ba46f511a851ab09998b5a5a9fdbb102545a3864cb993095e1745996873a7` |
| `OFL-Lexend.txt` | `ofl/lexend/OFL.txt` | `5da8505887d0fa7fe963445fd58852707fda34adfeb65af25c99d152bab285bd` |

Library total: 34 woff2 files, 909,772 bytes (about 890 KB); the largest is 58 KB. A label only
loads the faces it uses.

### Pixel grids

A font is flagged `pixel: {emPx}` in `font-catalog.ts` only when its outlines sit on a whole-number
grid, so that at `k × emPx` dots per em every edge lands on a whole dot. Measured on the bundled
files (printable ASCII, all on- and off-curve points): for each candidate `emPx`, the share of
coordinates on the grid of `unitsPerEm / emPx` font units.

| Family | unitsPerEm | Result | Flag |
|---|---|---|---|
| Silkscreen (Regular, Bold) | 1000 | 125-unit grid: 98 % of coordinates exactly on it, 100 % within 0.04 design pixel (the rest are 5-unit contour overlaps); every advance is a multiple of 125 | `pixel: {emPx: 8}` |
| VT323 | 1000 | rows on an 80-unit grid (12.5 per em, not a whole number), CRT-style dot bulges of 4 units drawn with curves, and half-pixel horizontal steps (e.g. x = 70, 100, 330); 39 % of coordinates exactly on the best whole-number grid (40 units, 25 per em) | none: drawn like any outline font |
| Pixelify Sans (Regular, Bold) | 1000 | optically sized "pixels" (stems 100–102 units, gaps 80–91 units); under 1 % of coordinates exactly on any grid from 4 to 40 per em | none |

VT323 and Pixelify Sans stay in the picker's Pixel category for their look; only Silkscreen is
snapped to whole-dot scales and drawn without anti-aliasing (P-size, `render/text.ts`).
