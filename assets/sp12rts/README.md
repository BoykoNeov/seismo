# SP12RTS mantle model

Whole-mantle 3D model of shear-wave (`SP12RTS..ES.sph`) and compressional-wave
(`SP12RTS..EP.sph`) velocity variations, by Paula Koelemeijer, Jeroen Ritsema, Arwen Deuss and
Hendrik-Jan van Heijst. SEISMO uses it to perturb the 1D ak135 model.

If you use these files, cite the paper, as the authors ask:

> Koelemeijer, P., J. Ritsema, A. Deuss & H.-J. van Heijst (2016). SP12RTS: a degree-12 model of
> shear- and compressional-wave velocity for Earth's mantle. *Geophys. J. Int.*, 204(2),
> 1024–1039. doi:10.1093/gji/ggv481

Source: the authors' download page, <https://users.ox.ac.uk/~univ4152/downloads_sp12rts.html>,
file `SP12RTS_plotting.tar.gz` (retrieved 2026-10-14). The two files are copied unchanged from the
root of that archive (identical to the copies in its `Models/` directory). The download page
states no licence; the files are bundled with this citation.

| File | SHA-256 |
|---|---|
| `SP12RTS..ES.sph` | `ceb039fef9e19c45c4ebb98b62abb95b1cd9f51e0a56984304acfac693667d6c` |
| `SP12RTS..EP.sph` | `4575805bff1714568b7240715dc7168b8aefd305da5a9cee537b57e8fbfa1baf` |

## Format

First line: maximum degree (12), a mask of the degrees present, the number of radial
parameters (24) and a mask of which are present (`000` then 21 ones: the three crust/topography
parameters are absent). Then, for each of the 21 radial splines (top of the mantle first) and each
degree l = 0..12, 2l+1 coefficients in Fortran `e12.4` format: m = 0, then cos and sin for m = 1..l.

- Lateral: real and imaginary parts of fully normalised complex spherical harmonics with the
  Condon–Shortley phase (orthonormal over the sphere), as in the authors' `ylm.f`/`legndr.f`.
- Radial: 21 cubic splines in x = -1 + 2(r - 3480)/(6346.619 - 3480), r in km; spline k is 1 at
  knot k and 0 at the others, with end slopes from three-point differences (`rspln.f`).
- Values are fractional velocity perturbations dlnV relative to PREM. Degree 0 (the radial
  average) is essentially removed by the authors; SEISMO ignores it.
