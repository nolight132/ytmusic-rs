# Vendored JavaScript

`src/deobf.rs` embeds these with `include_str!` and runs them in QuickJS to recover the
stream signature and the `n` parameter. A SHA1 over all four files keys the on-disk solver
cache, so replacing any of them invalidates it automatically — nothing to bump by hand.

| file         | origin                                                                   | licence   |
| ------------ | ------------------------------------------------------------------------ | --------- |
| `solver.js`  | [yt-dlp/ejs](https://github.com/yt-dlp/ejs), shipped as `yt.solver.core.js` | Unlicense |
| `meriyah.js` | [meriyah](https://github.com/meriyah/meriyah) `dist/meriyah.umd.min.js`   | ISC       |
| `astring.js` | [astring](https://github.com/davidbonnet/astring) `dist/astring.min.js`   | MIT       |
| `prelude.js` | this repo — polyfills `structuredClone`, which QuickJS lacks              | —         |

`solver.js` tracks YouTube's player, so it is the one that goes stale. Take a newer copy
from a current yt-dlp install:

```sh
find "$(dirname "$(readlink -f "$(command -v yt-dlp)")")/.." \
  -name yt.solver.core.js -exec cp {} assets/js/solver.js \;
```

The solver expects `meriyah` and `astring` as globals, which the UMD builds provide when no
module system is present. Keep using the UMD variants.
