# Fieldglass

Open a GRIB or NetCDF file, or a Zarr store, in Visual Studio Code and see
what's in it: every message listed with its parameter, level and time, and any
field drawn as a map you can reproject, probe and contour.

There's nothing else to install. You don't need Python, C libraries or a
compiler, because the extension ships a prebuilt binary for your platform.
Every decoder is written from scratch in Rust, which is what makes that
possible. It decodes every GRIB2 packing the WMO defines and turns
spherical-harmonic (spectral) fields back into a map.

![Fieldglass rendering a GFS temperature field reprojected onto an orthographic globe with coastlines](https://raw.githubusercontent.com/D0ubleD0uble/fieldglass/master/extension/media/screenshot.png)

## Getting started

Open any file ending in `.grb`, `.grib`, `.grib1`, `.grb1`, `.grb2`, `.grib2`,
`.nc`, `.nc4` or `.netcdf`. VS Code opens it in Fieldglass, which shows a
table of what the file holds: one row per message for GRIB, and the
dimensions, variables and attributes for NetCDF. From there, open a field in
the render panel to draw it as a map.

A file with another name can still be opened. Right-click it in the Explorer,
choose *Open With…*, and pick *Fieldglass Viewer*. If it isn't a format
Fieldglass reads, you get a hex and text view of its bytes instead.

A Zarr store is a folder rather than a file, so it opens a different way. Run
*Fieldglass: Open Zarr Store…* from the Command Palette and pick the folder.
It opens straight into the render panel.

## What it opens

### GRIB1 and GRIB2

GRIB is the format forecast centers share model output in. Fieldglass lists
every message's metadata and draws any field it can decode as a color image.
GRIB2 decodes every packing the WMO has defined, and the
[GRIB2 packing modes](https://github.com/D0ubleD0uble/fieldglass#grib2-packing-modes)
table lists them.

Some fields, called spectral fields, store waves instead of values on a grid.
Fieldglass turns global ones (spherical harmonics) back into a map and draws
them. The map is half a degree, which shows detail up to T359, so a finer field
is drawn at T359 and the panel says so ("shown at T359 of T1279"). Clicking a
point shows both the value on the map and the field's full-detail value there.
Limited-area spectral fields (bi-Fourier) aren't drawn yet.

Two messages can be drawn together as wind or current arrows.

### NetCDF

Fieldglass shows the full structure of both [classic](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec) and [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec) ([HDF5](https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t3.html))
files: their dimensions, variables and attributes. Pick a variable and two
axes to draw, then step through the rest, such as time or level, or play them
as an animation. Click a cell to plot the variable through it as a time
series, profile or transect, or pick two axes that aren't latitude and
longitude for a cross-section.

Grids with ordinary latitude and longitude axes work, and so do WRF and GOES
files, and ocean or satellite files that give a position for every cell.

### Zarr

Fieldglass opens [v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) and [v3](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html) stores. A store is drawn exactly as NetCDF is,
with the same slice picker, animation, line plots, projections and overlays,
and only the chunks a slice covers are read.

### What works where

Some things work the same way for every format. The three file formats, GRIB1,
GRIB2 and NetCDF, open from the file types listed above or through *Open With…*
for any other file, and are recognized from their first bytes either way. A
Zarr store opens from the Command Palette instead. For every format, the render
panel has the colormap picker (eight maps, each reversible, plus any GMT color
tables you import), linear and log10 color scaling, the point probe, zoom and
pan, and PNG export.

The table covers the rest, where the formats differ. A bare tick means a
feature works in full, and a tick followed by text means it works within the
limit given. Where a format has no entry, the feature doesn't apply to it.

| Feature | GRIB1 | GRIB2 | NetCDF | Zarr |
|---|:---:|:---:|:---:|:---:|
| Per-message metadata (parameter, level, time, forecast period) | ✅ | ✅ (sections 0 to 4) | ✅ (dims / vars / attrs) | ✅ arrays and their dimensions, in the render panel's picker |
| Grid description (lat/lon, Gaussian, polar stereo, Lambert) | ✅ | ✅ lat/lon ([3.0](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-0.shtml)), rotated lat/lon (3.1), Mercator (3.10), transverse Mercator (3.12), polar stereo (3.20), Lambert (3.30), regular and reduced Gaussian (3.40), Lambert azimuthal equal-area (3.140), space view (3.90), spherical harmonic (3.50), bi-Fourier (3.61–3.63), HEALPix (3.150) | ✅ regular lat/lon ([CF coordinate vars](https://cfconventions.org/cf-conventions/cf-conventions.html#coordinate-types)), WRF (Lambert, polar stereo, Mercator, unrotated lat-lon), GOES geostationary, curvilinear (2-D `lat(y,x)`/`lon(y,x)`) | ✅ as NetCDF |
| WMO lookups (parameter, center, level type) | ✅ [ON388 Table 2](https://www.nco.ncep.noaa.gov/pmb/docs/on388/table2.html) + every ECMWF local table · centers from the [WMO Common Code Tables](https://github.com/wmo-im/CCT) | ✅ [Code Table 4.2](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_table4-2.shtml) complete (1,387 parameters across 60 discipline/category tables) + tables 0.0 / 1.2 / 1.3 / 1.4 / 3.1 / 3.2 / 4.1 / 4.3 / 4.4 / 4.5 / 4.6 / 4.10 · centers from the WMO Common Code Tables · local tables for ECMWF, DWD and NCEP | n/a | n/a |
| Tabular metadata viewer | ✅ | ✅ (sections 0 to 4) | ✅ | ❌ |
| Value decoding | ✅ | ✅ see [GRIB2 packing modes](https://github.com/D0ubleD0uble/fieldglass#grib2-packing-modes) | ✅ classic ([CDF-1](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec)/[2](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#offset_format_spec)/[5](https://parallel-netcdf.github.io/doc/c-reference/pnetcdf-c/CDF_002d5-file-format-specification.html)) + [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec) / [HDF5](https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t3.html) (contiguous, compact, and chunked storage with deflate / shuffle / fletcher32 / zstd / szip) | ✅ [v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) and [v3](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html), [sharded](https://zarr-specs.readthedocs.io/en/latest/v3/codecs/sharding-indexed/index.html) or not, with blosc / gzip / zlib / zstd / LZ4 and the byte and bit transposes |
| Metadata editing | ❌ | ❌ | ❌ | ❌ |
| 2-D grid rendering with colormap | ✅ | ✅ any decodable message (see decoding row) | ✅ regular lat/lon (classic + NetCDF-4 / HDF5), WRF, GOES geostationary, curvilinear (ocean tripolar, satellite swath) | ✅ as NetCDF |
| Render-panel projection picker (8 targets) | ✅ | ✅ (any decodable message) | ✅ regular lat/lon + WRF / GOES geostationary + curvilinear | ✅ as NetCDF |
| Field compare (difference / sum / ratio / mean) | ✅ two messages | ✅ two messages | ✅ two slices of a variable | ✅ two slices of an array |
| Map overlays (coastlines, borders, lakes, rivers, lat/lon grid) | ✅ | ✅ (any decodable message) | ✅ regular lat/lon + WRF / GOES geostationary + curvilinear | ✅ as NetCDF |
| Contour lines | ✅ lat/lon family + projected grids | ✅ lat/lon family + projected grids | ✅ regular lat/lon + WRF projections + curvilinear | ✅ as NetCDF |
| Wind and current arrows | ✅ two messages as u and v | ✅ two messages as u and v | ❌ | ❌ |
| Zonal average | ✅ lat/lon, Gaussian and reduced grids | ✅ lat/lon, Gaussian, Mercator and reduced grids | ✅ regular lat/lon + WRF Mercator and lat-lon | ✅ as NetCDF |
| Animation, line plots and cross-sections | ❌ | ❌ | ✅ | ✅ |
| Resampling picker (nearest / bilinear) | ✅ | ✅ | ✅ regular lat/lon + WRF / GOES geostationary + curvilinear | ✅ as NetCDF |
| Export decoded field as CSV | ✅ matrix + `lat,lon,value` (lat/lon family + projected grids) | ✅ matrix + `lat,lon,value` (lat/lon family + projected grids) | ✅ matrix + `lat,lon,value` (georeferenced grids) | ✅ as NetCDF |

Legend: ✅ supported, and any text after it names the scope · ❌ not available · n/a doesn't apply to the format.

## The render panel

The render panel is where a field becomes a map. Each of the controls below
works the same way whichever format the field came from, except where it says
otherwise.

### Projections

The projection picker offers eight targets. Source draws the grid in its own
shape. The others are equirectangular, Web Mercator, orthographic, polar
stereographic, and three whole-world maps: Mollweide, Robinson and Equal
Earth. Most of them take a center you choose. Orthographic recenters on any
longitude and latitude, polar stereographic takes either pole with any
central meridian, and the three world maps take a central meridian.

These grids reproject into every target:

- GRIB1: latitude and longitude, Gaussian, rotated latitude and longitude,
  reduced (quasi-regular) latitude and longitude and Gaussian, Lambert, and
  polar stereographic.
- GRIB2: latitude and longitude, rotated latitude and longitude, Mercator,
  transverse Mercator, polar stereographic, Lambert conformal, regular and
  reduced Gaussian, Lambert azimuthal equal-area, and geostationary
  (space-view perspective).
- NetCDF and Zarr: regular latitude and longitude, WRF, GOES geostationary,
  and grids that give a position for every cell.

Spherical-harmonic fields from either GRIB edition, and GRIB2 HEALPix fields,
are turned into a latitude and longitude grid first, so they reproject too.

### Zoom and pan

On the equirectangular and Web Mercator maps, scroll over the map to zoom
around the pointer, and drag to pan, including across the date line. The
image follows the gesture straight away and is redrawn at full detail when you
let go. The Bounds row gains zoom in, zoom out and Reset view buttons, and each
gesture writes the new window into its latitude and longitude fields, so you
can finish by typing an exact one. A click without a drag still reads the
value under the pointer.

### Colors

There are eight colormaps: five sequential (viridis, plasma, cividis, turbo
and grayscale) and three diverging (red–blue, brown–teal and cool–warm), and
each can be reversed. Viridis is the default. Diverging maps suit anomaly and
difference fields, where the middle value is the one that matters. The colors
are applied in Rust, and the legend is drawn from the same table that paints
the grid.

The Log₁₀ toggle switches from linear scaling to base-10 log. It spreads a
field that spans orders of magnitude, such as precipitation, aerosol optical
depth or chlorophyll, evenly across the colors. Values of zero or below are
drawn as missing, and the toggle is grayed out unless the color range's
minimum is above zero.

### Your own color tables

*Fieldglass: Import Color Table…* in the Command Palette reads a GMT color
palette table (`.cpt`), the format that GMT, cpt-city, the Scientific Colour
Maps and cmocean publish palettes in. The table joins the Colormap menu under
Imported, in every render panel including ones already open, and stays there
in later sessions. Continuous and banded tables both work, with colors given
as `r/g/b`, `#rrggbb`, a gray level or an X11 color name. The table's colors
are spread across the display range the same way a built-in map's are; its own
value range isn't applied.

A table that uses another color model (HSV or CMYK), transparency, pattern
fills, or one key and color per line is refused, with the line it failed on,
and nothing is imported.

### Wind and current arrows

A GRIB file stores a wind or current as two messages, one for each direction.
The field you opened is one component, and the Vectors row picks the other.
Tick Arrows and choose the v component, and the arrows are drawn over the map
in the color and weight the other overlays use. You can set the spacing, or let
it fit about twenty-four arrows across the grid.

An arrow's length on screen depends only on its speed, so two equal speeds draw
equal arrows anywhere on any map. The arrow for the fastest flow is about as
long as the gap between arrows, up to a quarter of the map's shorter side, so
when you zoom in the arrows spread apart and keep their heads. A key in the
corner is an arrow labeled with a speed in the field's own units, and it goes
into an exported PNG with the arrows.

Each arrow points the way the flow runs on the map, so on an orthographic
globe or a polar view the arrows turn with it. Some files, including HRRR, NAM
and Canada's polar-stereographic winds, give their components along the grid's
axes rather than east and north. Those are turned through the grid's own
north, and the panel says so. It also says when the two messages aren't a u
and v pair, such as wind speed with wind direction, or are a pair picked the
wrong way round, though it still draws the arrows. A cell missing either
component draws no arrow.

### Cross-sections

The slice picker's X and Y selectors take any two of a variable's dimensions.
So a time–latitude Hovmöller diagram, a latitude–depth section or a
time–level plot is a matter of picking the axes. A pair that isn't the file's
own latitude and longitude isn't a map, since nothing places it on the Earth.
The panel says so, puts away the projection picker and the map overlays, and
labels the axes from their own coordinates instead: a [CF time](https://cfconventions.org/cf-conventions/cf-conventions.html#time-coordinate) as a date, and
everything else as its value with units. Files that hold nothing but
cross-sections, such as a moored time series of depth against time, draw the
same way.

### Animation

A NetCDF or Zarr panel with a dimension to step through, such as time, shows
an Animate row. It has first, back, play and pause, forward and last, a speed
from half a step to ten steps a second, and a Loop toggle. It starts on the
time axis, which it finds from the file's [CF metadata](https://cfconventions.org/cf-conventions/cf-conventions.html#time-coordinate) (`axis = "T"`,
`standard_name = "time"`, or units such as `hours since …`) or from a dimension
named `time`, and you can pick any other dimension instead.

While it plays on an automatic color range, every frame is colored on the
range of the frame you started from, so a color means the same value in every
frame. A start frame with the same value in every cell has no range to hold,
so then each frame sets its own. Each step waits for the one before it to
finish drawing, so a slow file plays slower rather than falling behind. Moving
a slice slider by hand stops playback.

Each slice stepper shows the coordinate at its index beside the number, such
as 500 millibar or a date, and so does the Animate label. The line under the
heading names the slice drawn, so an exported PNG says which one it is.

### Line plots

In a NetCDF or Zarr panel, tick Line plot, and a click that reads the value
under the pointer also draws a line plot below the readout. It shows the
variable through that cell along an axis you choose: a time series, a
vertical profile or a transect. The box starts unticked, so a click only reads
the value until you ask for the plot. Points are placed by the axis's own
coordinates and labeled with their units, gaps stay gaps, and a dashed line
marks the slice on screen. Only the chunks the line crosses are read.

### Zonal average

The Zonal average checkbox draws the field's mean along each row of latitude
as a line below the map. It works for regular latitude and longitude,
Gaussian, Mercator and reduced grids in any format. On a reduced grid only the
values each row stores are averaged, and a row with no data is a gap. Rotated
and projected grids have no rows of latitude, so there the checkbox is hidden,
and the "Not available for this grid" line under the grid caption says why.

### Comparing two fields

The Compare row picks a second field and an operation: A − B, B − A, A + B,
mean(A, B), or A / B. A difference map is the classic way to read an anomaly,
a bias, or the change between two time steps. For GRIB, the second field is
another message in the file. For NetCDF and Zarr, it's the same variable at a
second slice, with its own dimension steppers. A cell missing in either field
is missing in the result, and the two fields must be on the same grid. The
result is drawn like any other field, so the projection, overlays, colors and
scaling all apply.

### Reading a value

Click anywhere on a field to read the point under the pointer, with the
decoded value and its units there. The readout uses the same pixel-to-point
mapping as the picture, so the number is exactly the one the color shows, on
every projection. A click off the globe reports nothing.

### Contour lines

The Contours toggle draws lines of equal value over the field, and they follow
the map through every projection. Fieldglass picks round levels (1, 2 or 5
times a power of ten), or you can set an interval. Lines break cleanly around
missing data. A "contours only" mode hides the colors so the lines read on a
blank background. The lines are traced on the grid and then placed through the
same mapping the picture uses, so they line up with the data.

Contours work on the latitude and longitude family (regular, Mercator,
rotated, regular and reduced Gaussian, and reduced latitude and longitude) and
on the projected grids (Lambert conformal, polar stereographic, transverse
Mercator and Lambert azimuthal equal-area). On a geostationary scan-angle grid
the panel says contours aren't available, instead of drawing them wrong.

### Map overlays

Five layers can be drawn over any field: coastlines, country borders, lakes,
rivers, and a latitude and longitude grid. Each one is toggled on its own and
has its own color and line weight, so you can tell a coastline from a border
at a glance. The four map layers are Natural Earth 1:110m data, bundled with the
extension, so nothing is fetched while you work. They're projected in Rust
through the same mapping as the field, so an overlay can never drift from the
picture under it.

## Limits

Fieldglass is a viewer, and it doesn't edit a file's metadata. The Rust
library can patch a GRIB1 message's forecast period byte by byte, but the
viewer doesn't offer it.

The extension reads a whole file into memory, so that remote and virtual
workspaces keep working. That makes archives of several gigabytes a poor fit.
A Zarr store is the exception, because it's read one document or chunk at a
time.

A few formats and grids fall outside what's drawn:

- GRIB1 stretched grids, oblique Lambert, any other grid type not listed
  above, and predefined grids other than [ON388 Table B](https://www.nco.ncep.noaa.gov/pmb/docs/on388/tableb.html) grids 2, 3 and 4 are
  listed but shown as `unsupported`.
- GRIB2 bi-Fourier fields decode to their coefficients but aren't drawn.
- GRIB1 local parameter tables from centers other than ECMWF aren't included.
  A field from one is shown as `Parameter center/table/id`, naming the codes,
  instead of by name.
- Rotated WRF latitude and longitude domains aren't placed.
- HDF4 and HDF-EOS2 files aren't read.
- A NetCDF-4 variable stored as a compound, enum, variable-length, opaque or
  array type is listed under "Variables not decoded" with the type that
  stopped it, and the rest of the file reads as usual.
- A Zarr store has no metadata table. [Kerchunk references](https://fsspec.github.io/kerchunk/spec.html) and remote
  stores can be read through the Rust library, but not from the extension.

Lambert and polar-stereographic grids are placed on the Earth shape the file
declares, which matches an independent decode point for point for the
spherical shapes nearly all data uses. A grid that declares an oblate
spheroid is projected on that spheroid's mean radius instead, which is within
about 0.1%. Transverse Mercator and Lambert azimuthal equal-area are the
exceptions: they run on the declared spheroid, because the mean radius would
place a UK-area field about 2.8 km out and a European one about 13.5 km out.

## Installing

Search for "Fieldglass" in the Extensions view and install it from the
[Marketplace](https://marketplace.visualstudio.com/items?itemName=fieldglass.fieldglass).

To install a particular release by hand, download the `.vsix` for your
platform from the [releases page](https://github.com/D0ubleD0uble/fieldglass/releases/latest).
There's one for each supported platform, named like
`fieldglass-linux-x64-x.y.z.vsix`:

- Linux x64 and arm64 (glibc)
- macOS x64 and arm64
- Windows x64 and arm64

Then run *Extensions: Install from VSIX…* from the Command Palette and pick the
file, or install it from a terminal:

```sh
code --install-extension fieldglass-linux-x64-x.y.z.vsix
```

Reload the VS Code window when it's done. Fieldglass needs VS Code 1.85 or
newer.

## Attribution

The bundled colormaps are third-party color specifications, used under their
own terms:

- This product includes color specifications and designs developed by Cynthia
  Brewer (<https://colorbrewer.org/>). The red–blue and brown–teal diverging
  maps are from ColorBrewer, © Cynthia Brewer, Mark Harrower and The
  Pennsylvania State University, licensed under the Apache License, Version
  2.0.
- The turbo colormap is © 2019 Google LLC (Anton Mikhailov), licensed under
  the Apache License, Version 2.0.
- Viridis, plasma and cividis are from matplotlib and are released into the
  public domain (CC0) by their authors: Nathaniel J. Smith, Stéfan van der Walt
  and Eric Firing (viridis, plasma), and Jamie R. Nuñez, Christopher R.
  Anderton and Ryan S. Renslow (cividis).
- The cool–warm diverging map is by Kenneth Moreland and is released into the
  public domain (CC0).
- The color names an imported color table may use are the X11 color database
  (`rgb.txt` from X.Org `app/rgb`), © 1985, 1989, 1998 The Open Group, used
  under the X11 permission notice in that repository's `COPYING`.

The coastline, border, lake and river overlays are Natural Earth 1:110m data,
which is in the public domain.

## More

Fieldglass is open source, under MIT or Apache-2.0 at your option. The
[project page](https://github.com/D0ubleD0uble/fieldglass) has the Rust crates
and the browser package the extension is built on, the
[changelog](https://github.com/D0ubleD0uble/fieldglass/blob/master/CHANGELOG.md),
the [roadmap](https://github.com/D0ubleD0uble/fieldglass/blob/master/ROADMAP.md),
and the place to [report a problem](https://github.com/D0ubleD0uble/fieldglass/issues).
