# CPT fixtures

Real colour palette tables for `tests/cpt_real_files.rs` (#236), copied
unmodified from GMT's `share/cpt` at commit
`e933356f7726b314d2d757a09b8c8d3eb44ce63a`
(<https://github.com/GenericMappingTools/gmt/tree/e933356f7726b314d2d757a09b8c8d3eb44ce63a/share/cpt>).

GMT itself is LGPL, but these tables are not GMT's work: GMT redistributes them
under their authors' own licences, which each file's header states. All four
here are MIT. GMT's own tables (`haxby` and the rest of `share/cpt/gmt`) were
passed over for that reason.

| File | Source in `share/cpt` | Shape | Author and licence |
|---|---|---|---|
| `batlow.cpt` | `SCM/batlow.cpt` | 255 linear slices, `B`/`F`/`N` | Fabio Crameri, Scientific Colour Maps 8.0.1, MIT |
| `vik.cpt` | `SCM/vik.cpt` | 254 linear slices across -1..1, hinge at 0 | Fabio Crameri, Scientific Colour Maps 8.0.1, MIT |
| `batlowS.cpt` | `SCM/batlowS.cpt` | categorical, 100 keys | Fabio Crameri, Scientific Colour Maps 8.0.1, MIT |
| `balance.cpt` | `cmocean/balance.cpt` | 256 flat bands across -1..1 | Kristen M. Thyng, cmocean 2.0, MIT |

Crameri, F. (2023). *Scientific colour maps* (8.0.1). Zenodo.
<https://doi.org/10.5281/zenodo.1243862>. Copyright (c) 2023 Fabio Crameri.

Thyng, K. M., Greene, C. A., Hetland, R. D., Zimmerle, H. M., & DiMarco, S. F.
(2016). True colors of oceanography. *Oceanography*, 29(3), 10.
Copyright (c) 2015 Kristen M. Thyng.

Both are distributed under the MIT License:

> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in
> all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.
