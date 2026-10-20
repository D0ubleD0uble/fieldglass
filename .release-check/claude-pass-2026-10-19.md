# Claude pass (automated, Xvfb), master 957ef367, 2026-10-19

## Results
§1.1 gfs renders, reprojects, coastlines align: PASS
§1.2 gfs controls (no Resampling on Source, no Not-available line): PASS. mirs BT: Zonal hidden, line reads as specified, starts closed, opens on click: PASS (keyboard path not tried)
§1.3 Resampling only on warp; Bilinear survives Source and back; gfs map identical (0 px diff): PASS. mirs: persists, map identical: PASS — but see C1
§1.4 Contours gfs Mollweide: no streaks: PASS
§1.11 Export PNG (gfs Mollweide+coast+contours): default name gfs_temperature_message-0.png, 3006x1514, matches screen: PASS
§1.12 Export CSV beside PNG on render panel: layout prompt, default samples/gfs_temperature_message-0.csv, 1038240 rows, range 210.43..309.67 = panel: PASS

## Findings
C1 (minor) mirs_swath BT (curvilinear) on Equirectangular: Resampling picker offered, Bilinear changes nothing (map pixel-identical to Nearest) and caption keeps "(nearest)". Code downgrades cell-centre grids by design (api.generated.ts resampling doc), so per #1036 the picker should be hidden with a "Not available" reason.
§1.5 spectral T63 renders: PASS. §1.6 sizes T63 / Nside 4 / N4 M4: PASS
§1.7 HEALPix Orthographic: smooth disc edge, coastlines follow, data blocky: PASS
§1.8 classic dummy metadata: PASS. §1.9 regular_latlon renders: PASS
§1.10 NetCDF diff mirs BT ch0−ch10: probe 70.125 at grid 55,131 = 282.0−211.875: PASS (probe is click, not hover)
§1.10b GRIB diff gfs_wind u−v: probe 4.6204 @26.25N 176.5W = −2.2811−(−6.9015) (eccodes): PASS
§1.13 tooltip text + prompt "Compare is on: the CSV holds Field A, not the difference. CSV layout"; CSV value at that point = u (−2.2811): PASS
§2.0 cold window (Reload, no file open) → Open Zarr Store: folder dialog, no "command not found": PASS (Import Color Table: see below)
§2.1 air_v2 opens on air 144×73, poles cold; coastlines align Equirect + Ortho: PASS
§2.2 time 366 steps, stepping changes map: PASS. §2.3 t=14 probe 40N 100W = 246.60 degK grid 104,20: PASS
§2.4 stepper "2024-01-15", heading line "air_v2.zarr · time 2024-01-15": PASS
§2.5 Export CSV beside PNG: PASS. §2.6 long 10512 data rows; matrix 73 rows: PASS
§2.7 CSV dialog in store folder on air_v2_air_time-2024-01-15.csv; "out" → out.csv: PASS
§2.8 PNG offers air_v2_air_time-2024-01-15.png, step → …-01-16.png; out → out.png: PASS
§2.9 v2 / v3 / NetCDF t=14 all 246.60 at 104,20; exported PNGs identical except subtitle text: PASS
§2.10 tabs "air · air_v2.zarr" etc., heading "air (degK)", no "Message"; time_bnds tab "time_bnds · ncep_air_daily_2024.nc", PNG offers ncep_air_daily_2024_time_bnds.png: PASS — but C2
C2 (moderate) ncep_air_daily_2024.nc → time_bnds (2×366, Y=time X=nbnds): on-screen image is a blank white box with a broken-image glyph, ~140k px tall (canvas max-width:100%, height:auto → 775 px wide × 183:1). Probe and colorbar work, Export PNG is correct (2-px strip). A very thin 2-D variable needs a capped display aspect.
§2.11 air_copy (no .zarr suffix) opens the same: PASS
§2.12 cf_v3 t physical (240.0..290.5 = 200+0.25×raw 160..362); v2_slash draws: PASS
§2.13 v3_v2keys axes phony_dim_0/phony_dim_1, no stepper: PASS. §2.14 v3_sharded s 7×5 draws: PASS
§2.15 v2_problems warning + Details dialog as specified, no panel: PASS (toast has an extra explanatory sentence after the first, then the reasons)
§2.16 "stores" message + Details: PASS. §2.17 fixtures: 36 stores, first 8, "and 28 more"; src: "isn't a Zarr store. Pick the folder that holds the store's data": PASS
§2.18 210 folders + zzzz: "looked at only the first 200 folders… found no Zarr store"; Details adds name-order note: PASS
§2.0b cold window Import Color Table: file dialog, no error: PASS
§3.1 Animate row (first/back/play/fwd/last, speed, Loop), starts on time: PASS
§3.2 icons |◀◀ |◀ ▶ ▶| ▶▶| same height, centred, mirrored; play→two bars, same size; Light Modern / Dark Modern buttons take theme colours: PASS
§3.3 Tab from heading: Export PNG, Export CSV, selects, … Animate buttons, each ringed; Dark High Contrast: every button bordered: PASS (zoom buttons not reached: Source projection has none)
§3.4 Play counts "2024-01-0N (step N of 0–365)", stepper + heading date follow: PASS. NH warming towards July not judged by eye.
§3.5 range held during playback (231.8..310.3 throughout): PASS — see C3
§3.6 5/s ≈ 5 steps/s, 0.5/s slower; Loop on 365→0; Loop off stops at 365: PASS
§3.7 5/s + Loop off + Zonal on survive switching to README tab and back: PASS
§3.8 levels: level 5 "500 millibar", label "500 millibar (step 5 of 0–16)", heading "· level 500 millibar", PNG subtitle same; time reads 0000-12-30 (known gap). Equator probe 100 hPa −82.3 °C, 10 hPa −45.1 °C: PASS
§3.9 Compare A−B, Field B level 2: "Field B · time"/"Field B · level" 850 millibar, orange rule, faint tint, "·" gap; heading "· Field B at level 850 millibar": PASS
§3.10 large_sparse t2m: no-data steps "no values" no colour bar; step 7 bands −90..90, step 119 10..190; no stall: PASS
§3.11 gfs file page Render / Export CSV… theme colours, Tab ring, HC border: PASS
(Own error, not a bug: closing the file's metadata tab while the render panel is open gives "NetCDF handle was disposed" — that is §22's expected behaviour.)
C3 (minor, observation) After pausing playback, the paused frame keeps the held playback range (231.8..310.3 at step 3). Switching tab away and back re-renders the same step with its own range (227.0..311.9). Same frame, two colour bars depending on an unrelated tab switch. Worth a decision: release the hold on pause, or keep it until the next manual step.
§4.1 Line plot box beside Zonal, unticked; click → readout only: PASS
§4.2 tick → plot at once; 60N 100E series cold Jan/warm Jul; "air at this point, across time"; caption exactly "air across time (hours since 1800-01-01 00:00:0.0) · 238.35 to 300.83 degK · 366 of 366 points"; date ticks 2024-01-01…2024-12-31; no labels touch: PASS
§4.3 tab away/back still ticked; untick → click reads only: PASS (the plot itself is cleared on return, checkbox kept)
§4.4 dashed rule at day; step → plot clears; re-click → rule at 2024-06-29: PASS
§4.5 levels 30S 25E (31.48 °C) along level: upright, 1000 bottom/10 top, log spacing (100 halfway), coldest ≈100 mb, true minus (−50), rule at 1000: PASS
§4.6 caption "−72.8 to 31.5 degC · 17 of 17 points": PASS
§4.7 large_sparse along time: 2 points (7, 119), no line: PASS (both points sit on the plot frame: one on the x axis line, one at the far right edge — cosmetic)
§5.1 Mollweide + Bilinear set: done
§5.2 time(X)/lat(Y): "Cross-section: these axes are not a map, so it is drawn in index space."; projection, Resampling, overlays, Zonal hidden and named in the Not-available line (+ Long layout); time–lat plot, lat ticks 90…−90, date ticks: PASS
§5.3 end labels don't overprint, 2024-12-31 at image edge, 90 clear of Line plot box; narrowed panel drops labels, none overlap: PASS
§5.4 click: "300.20 degK · grid 192,31", no longitude; lon is offered in the Line-plot picker: PASS (picker defaulted to time, the panel's saved preferredLineAxis, by design)
§5.5 air_v2.zarr time/lat: date ticks: PASS (lat ticks read 90, 55, 17.5, −17.5, −55, −90, not round numbers — cosmetic)
§5.6 back to lon/lat: Mollweide + Bilinear restored: PASS
§5.7 time/lat, tab away/back, lon/lat: still Mollweide: PASS
§5.8 levels lat(X)/level(Y): 1000 at foot, 10 top, warm at bottom; Flip Y turns it over: PASS
§6.1 gfs_wind UGRD + V=#1 VGRD: arrows, key lower-left "18.1 m s⁻¹", row "Full-length arrow: 18.1 m s⁻¹", no frame note, no pair note; every 80 → fewer longer arrows, key lengthens (18.8): PASS
§6.2 msg 1 VGRD + V=#0 UGRD: arrows + wrong-way-round note exactly; untick → note gone: PASS
§6.3 colour red + weight 3: arrows + key redraw; PNG has arrows + key with readable label; untick → arrows, key, "Full-length" text gone: PASS
§6.4 zoom + twice: key lengthens each time; Orthographic: key shorter with arrows; zoom to 0.01° window: arrows stop growing, one arrow in view has its head, key whole in corner with row speed 18.1; PNG there: key inside map; narrow panel → "No arrow key: the map is too small to fit one.": PASS. No arrow longer than key on Equirect/Ortho: PASS by eye.
§6.5 spacing 4, red: survives tab switch and batlow.cpt import: PASS
§6.6 westerlies east, trades west; anticlockwise flow around the NW-Pacific storm: PASS
§6.7 no band of arrows along top/bottom edge (auto, and after Reset view): PASS
§6.8 priority: Ortho centre lat ±90 and Polar stereo N/S: arrows follow flow around pole, no fan at pole, heads square, edge arrows whole: PASS
§6.9 hrrr_wind msg 0: "This file states its components along the grid's axes; they are rotated to true north.": PASS
§6.10 priority HRRR vs GFS on the same window (21.1–52.6N, 134.1–60.9W): directions agree on the west coast (northerlies off CA/OR, onshore into the Baja low) and east coast (onshore easterlies into the Carolinas, NE-ward in New England), edges as well as middle: PASS by eye
§6.11 cmc_pair: source view all arrows same direction; Equirect fan out with longitude: PASS
§6.12 cmc_pair note "These messages aren't a u/v pair (WIND, WIND). …" under the axes note: PASS
§6.13 gfs_unnamed_pair: table Parameter 0/2/250, 0/2/251; arrows draw, no pair note: PASS
§6.14 hrrr_frames 0+1 refused "…component frame differs (u: grid-relative, v: earth-relative)", nothing drawn: PASS
§6.15 0+2 draws with "aren't a u/v pair (TMP, TMP)": PASS
§6.16 refusal gone on #2, back on #1, Contours keeps it, Untick clears it: PASS
§6.17 refusal on its own line under the summary; survives colour map change: PASS (message change not tried)
§6.18 gfs_wind export cancel → "PNG export cancelled." below summary; save → "Exported PNG to <path>"; colour-map change empties it: PASS
§6.19 two_grids 0+1: "the two fields are on different grids … raster shape differs (A: 1799x1059, B: 16x31)": PASS
§7.1 gfs Equirect + Web Mercator: scroll zooms around pointer (pointer at 12.6E 35.6N stays there), drag pans; image follows during gesture, re-renders after: PASS
§7.2 pan across date line (window 70.9..246.3): continuous, no strip: PASS
§7.3 +/−/Reset in Bounds row; fields show window after gestures; typed Europe window works: PASS
§7.4 Europe Export PNG = zoomed window: PASS
§7.5 click probes; drag released off image then click → probes: PASS
§7.6 Mollweide, Orthographic (and Polar): scroll/drag leave map unchanged (0 px diff mid-drag): PASS
§7.7 Flip Y: zoom near top edge keeps place under pointer (lat −77.6 → −77.9, lon 237 → 236.3); drag moves map with pointer: PASS
§7.8 priority mrms race: FAIL as written — see C4
§7.9 mrms Mollweide + immediate scroll: Bounds stays Auto; back to Equirect draws whole map: PASS
C4 (moderate, plan or code) §7.8 cannot pass by hand. The napi render is synchronous on the extension host, so a click while the old MRMS image is up is processed only after the Mollweide render finishes. Its reply then arrives after the render result, which bumps probeSeq, so the reply is dropped as stale (render-panel.ts:2824, 3686). Seen: 3 tries, a click on the old image (centre or corner) never gave a readout. A readout from an earlier click stays on screen during "Rendering…" though the new click is ignored. Either the plan check should say this path is test-only (axes.test.ts holds the render back), or the click needs answering against the drawn image without waiting on the host (e.g. a webview-side lookup or an async render).
§7.10 priority gfs_wind Europe zoom + pan with coastlines, contours, arrows: all line up with the field after the gesture: PASS
C5 (minor UX) The first wheel tick or drag on an Auto-bounds map ticks Bounds Manual, which shows the lat/lon fields row. That pushes the map down ~58 px mid-gesture, so the next wheel ticks zoom around a different place than the one first under the pointer. Seen: 5 ticks over 10E 50N ended on a 7–116E window, Europe at the left edge. One tick is right. Repeated ticks drift.
§7.11 ncep slice Equirect: zoom/pan work, Reset view returns whole globe: PASS
§8.1 gfs zonal: ~300 K tropics, coldest Antarctica, "721 of 721 points": PASS
§8.2 ncep zonal follows the slice (Jan vs Jun 29): PASS
§8.3 reduced_gg_n32_smooth: "64 of 64 points", 248.06..287.98 K, no polar gap: PASS
§8.4 hrrr: checkbox hidden; line opens to "Zonal average: this grid's rows aren't circles of latitude.": PASS
C6 (cosmetic) Latitude axis ticks on the zonal plot and the cross-section use raw coordinate values: "-87.8638, -71.1578, -6.97653" (reduced gg), "-80.5, -71, -61.5, -52, -42.75" (gfs), "90, 55, 17.5, −17.5" (air_v2 cross-section). Round ticks (−90, −60, −30, 0…) would read better.
§9.1 batlow (dark blue→pink), vik, balance (diverging) under Imported, in the open panel and a new one: PASS
§9.2 survive Developer: Reload Window: PASS
§9.3 batlowS refused "line 15: this is a categoric…" (toast truncated; wording matches cpt.rs), nothing added: PASS
§9.4 hsv.cpt refused "line 1: colour model "HSV" is…": PASS
§10 hdf5_szip.h5: all 12 datasets draw a source view, no error; hdf5_szip_hand.h5: rsi1_f4, rsi1_i2 draw: PASS
§11 grib1_names: msg 0 2 metre temperature; 1 Significant height of combined wind waves and swell (m); 2 Parameter 78/129/167; 3 wave height again: PASS
§12 msg 0 Level Type "Ground or water surface", Level "—": PASS
§13 msg 4 "Level type 250", msg 5 "Missing"; grib2_codes msg 1 "Parameter 99/0/0"; ecmwf_lfpw Center "… Météo-France (Sub-centre 105)": PASS
§14 grib2_codes msg 0 "Stochastically Perturbed Parametrization Tendency (SPPT)", units empty in table, title, colour bar: PASS
§15 GRIB2 Center ends "· Operational products"; GRIB1 none; no Unknown/Reserved/N/A; dashes only in GRIB1 Level for surface/Missing: PASS
§16.1 priority spectral_pair subtitle "shown at T359 of T383": PASS. "smooth, no speckle": cannot be judged — see P1. No stripes, rings, blank patches.
§16.2 priority probe shows map + full-detail value; at 0N 0E: 320.26 K shown · 308.37 K full = oracle exactly (truncated 320.264, full 308.369): PASS — but "close" in the plan — see P1
§16.3 Compare A−B: zero everywhere (range 0.000…0.000), label kept: PASS
§16.4 PNG carries label; CSV starts "# spectral field shown at T359 of T383: …"; first value 283.5336 = oracle map[0]: PASS
P1 (plan) §16: spectral_simple_t383 is built from seeded white-noise coefficients (flat spectrum, NOTICE.md), so the map IS speckle at every scale, and the truncation alone moves a point by ~10 K std (oracle: 12 points, full−truncated σ = 10.6 K; 38.6 K seen at 51S 48W). "Smooth, with no speckle" and "the two are close" can't be judged honestly. Suggest: check the probe at 0N 0E against the oracle (320.26 shown / 308.37 full), and drop "smooth".
§17 ecmwf.grib2 global 2 m T, coastlines aligned; ccsds_flags46_12bit draws global, max 314.7: PASS
§18.1 dimscale temperature: projection picker offers map projections: PASS
§18.2 lat_bnds: picker + Resampling hidden, caption "source", line "Not available for this grid: Projection, Resampling, Overlay, Zonal average, Long layout (Export CSV)": PASS
§18.3/18.4 back to temperature: picker returns, line gone; Equirect kept across lat_bnds (#1099): PASS
§19.1 lat_bnds reason "…: this grid has no known position on the Earth.": PASS. §19.2 polar_stereographic_surface: same reason: PASS
§20.1 coastlines + contours on, lat_bnds: Overlay row hidden, listed with the same reason, no error: PASS
§20.2 back: Overlay row returns with coastlines + contours on: PASS. §20.3 polar_stereo: Overlay hidden, same reason, no Vectors row: PASS
(own slip: an Export PNG click saved crates/fieldglass-netcdf/tests/fixtures/.png.png; deleted, tree clean)
§21.1 GOES fixtures + goes.nc placed: projection picker + coastlines offered; coastlines/borders draw on goes.nc: PASS
§21.2 Contours + Zonal hidden; line "Not available for this grid: Zonal average, Contours, Long layout (Export CSV)"; reasons in order (grid's own first); Tab reaches summary with outline, Enter opens: PASS
§21.3 Export CSV: Matrix only, prompt "CSV layout. The long layout isn't available for satellite-view grids yet.": PASS
§21.4 no "no position" claim, no error: PASS. §21.5 hrrr contours offered + draw, only zonal hidden: PASS
§22.1 copy open, panel on temperature, file editor closed, pick lat_bnds: "Error: NetCDF handle was disposed"; picker + overlays hidden (follow lat_bnds); no Not-available line: PASS
§22.2 overwrite with netcdf4_grouped, reopen, Reload Webviews: list is new file's; "Error: lat_bnds is no longer in the file, so nothing was drawn. Pick a variable to draw.": PASS — see C7 (list also has PRODUCT/latitude, PRODUCT/longitude, which the plan omits)
C7 (minor) After §22.2 the heading reads "PRODUCT/SUPPORT_DATA/surface_altitude (m)" though nothing was drawn and the Variable picker is blank. The GRIB counterpart (§23) reads "No message drawn". The heading names a variable the panel did not draw.
C8 (minor) netcdf4_grouped file page: the Variables and Dimensions tables write "/PRODUCT/SUPPORT_DATA/surface_altitude", "/PRODUCT/scanline" with a leading "/", while the Render buttons and the panel write "PRODUCT/…" without (#704 says no leading /). §25 is satisfied for the panel, not for the file page.
§23.1 members copy: both rows list parameter, level, forecast: PASS
§23.2 msg 1 + Compare Field B msg 0; swap to 8_7, reopen, Reload Webviews: draws "Message 0" (member 8), Field B #1 (member 7), tab "msg 0 TMP": PASS
§23.3 swap to 8_8: heading "No message drawn", tab "pick a message · members.grib2", "Error: This message is no longer in the file (or is ambiguous); pick one."; message list offered; pick #1 → "Message 1 — Temperature", tab "msg 1 TMP · members.grib2": PASS — but C9
C9 (moderate) After §23.3's pick, Operation still reads A − B but Field B and V component pickers are blank, and the summary says "A − B · 16×31 · range 270.5 … 311.1": Field A's own range (270.5..311.1 is msg 1's temperature) labelled as a difference. Either re-resolve Field B, or drop Compare to off / ask for Field B, rather than label A as A − B.
§24 cmc_pair Equirect bounds 50–30S 60–90E: nothing drawn, colour bar keeps 0.2096..75.21 (not 0–1), arrows on: no key, no "Full-length arrow": PASS
§25 netcdf4_grouped panel variables "PRODUCT/qa_value", "PRODUCT/SUPPORT_DATA/surface_altitude", no leading "/": PASS (file page differs — C8)
§26 hdf5_phony_dims a_8x8 axes phony_dim_0, phony_dim_1: PASS
§27.1 large_sparse (40 KB) opens at once; step 7 drew within 0.8 s: PASS
§27.2 fill_only_at_field_cap 8192×8192: "8192×8192 · no values", no colour bar (~10 s): PASS
§27.3 fill_only_past_field_cap refused "this read asks for 100000000 values at once, more than the 67108864 one field may hold. The file itself is fine": PASS — C10
C10 (cosmetic) Render failures read "Error: render failed: Error: this read asks…" — "Error:" twice. A failed render also leaves an empty colour bar labelled "—" beside the empty map.
§28 mrms Mollweide map 11585×5792 (~25 s, no long stall beyond that); Export PNG 11711 × 5866: PASS
§29 userblock / no-userblock both open (Render c, v; dims x,y), c draws identically (0 px diff): PASS
§30 unsupported_type opens; station_info under "Variables not decoded" with "HDF5 datatype class 6 (compound)"; time, temperature listed: PASS — P2
P2 (plan) §30 says `visits` is "listed as usual"; it is under "Variables not decoded" with class 9 (variable-length), which is what hdf5_unsupported_datatype.rs:101 expects. Plan wording only.
§31 sizes_o4_l8, sizes_o8_l4, soft_links_earliest, huge_attributes, huge_link_name (Empty NetCDF dataset, no variables), btree_k_sb1/sb2 (v listed): all open, no "could not be fully resolved" note: PASS. history cell shows first 120 chars + "…": PASS. Hover tooltip: see C11
C11 (minor) netcdf4_huge_attributes: hovering `history` shows ~15 lines (~1,900 chars) ending "ncks -O…", not all 5,600. The page puts the full value in title= (provider.ts:2850), but the native Chromium tooltip truncates it. Plan expects all 5,600. Needs another way to read a long value (expand/copy), or the plan should say what the tooltip shows.
§32 ext-host RSS flat at 348–349 MB throughout; every message within ~1 s:
  chunk_past_ceiling: deflate_f8_bomb + zstd_f8_past_ceiling "…chunks that decode to 320.0 MiB each… The file itself is fine"; zstd_f4_past_field_cap "…268435456 values at once… The file itself is fine"; deflate_f8_unwritten flat 0: PASS
  szip_hostile: both refused: PASS. codec_bombs: deflate/zstd "…past the chunk's 3145728-byte length"; short streams "chunk decoded to 64 bytes, expected 268435456": PASS
  shared attribute/link/local-heap: superblock view, notes as specified incl. "The file itself is fine"; shared_continuation "object-header chunk at 98376 overlaps one already read", no "fine": PASS
  deep_group_chain opens at once, lists v under its 600-group path: PASS
  fixed_array_huge_count "17179869184 values at once… fine"; zstd_wide_window "64.0 MiB of history… 8.0 MiB… fine": PASS
  numrecs_huge_cdf1/cdf5 Render v refused "…exceeds source size 248" / "364": PASS. "v lists 60,000,000 records": not visible — the page shows time Length "unlimited"; no count anywhere (P3)
P3 (plan or UI) §32 numrecs_huge: plan says v lists 60,000,000 records, but the file page shows the record dimension as "unlimited" with no count, and the refused panel shows none. Either show the stated count or fix the plan line.
§34 record_single_short s 3×3: range 1..9, nine cells stepping evenly from one corner: PASS
§34a streaming cdf1/cdf5: v 1×2, range −8..7; partial cdf1/cdf5: v 1×1, 7: PASS
§35 priority j_consecutive GRIB1 and GRIB2 on Equirect: corners 0 (grid 0,0), 7 (7,0), 40 (0,4), 47 (7,4); 8×5 picture: PASS
§36 priority reduced-grid pairs (GRIB1 second_order ± boust, row_by_row ± boust; GRIB2 second_order ± boust) render pixel-identical within each pair, no comb; row_by_row stripes are the fixture's flattened rows (NOTICE, eccodes row means agree): PASS
§37 reduced_gg_n32 long CSV 6114 data rows; matrix offered and exports (64 rows): PASS
§38 priority gfs Equirect default PNG: map cols 14..1453 (1440), first and last columns are field at rows 100/400/700, no background stripe: PASS
§39.1 polar_stereographic_surface source view flat 273.15 K; table Bounds "—" (no corner): PASS. Caption check: see P4
§39.2 Overlay row hidden, long CSV not offered ("CSV layout. The long layout needs a grid placed on the Earth, and this one isn't."), no "unsupported": PASS
§39.3 tiny_earth_pair: picker, Resampling, Overlay, Vectors, Zonal hidden; line + reason "…: this grid has no known position on the Earth."; no satellite-view mention, no error: PASS (list also has "Long layout (Export CSV)", not in plan text)
P4 (plan) §39.1 expects the caption "polar_stereo 16×31 (grid coordinates)". That is describeProjection's text in the page's first HTML (provider.ts:948), but the first render replaces it with "source: polar_stereo 16×31 → polar_stereo (no reprojection)", which has a "→". The no-corner part holds (table Bounds "—"). The plan should name the line the user sees.
§40 priority goes_geostationary vs _metres on Equirect + coastlines/borders: same 6×6 patch at the equator near 75W with the Ecuador/Colombia coast at its west edge; maps pixel-identical; probe same spot: 132.00 W m⁻² sr⁻¹ µm⁻¹, grid 2,3 in both: PASS
§41 rtofs_ice first draw ~5 s (Source). Source view: palette repaint <1 s, variable change ~2 s. Equirect: FAIL — C12
C12 (moderate) rtofs_ice (4500×3298 curvilinear) on Equirectangular: every palette change, variable change on the same grid, and projection switch shows "Rendering…" for ~6 s (status-line bursts: 6.3 s palette, 6.35 s Source→Equirect, 6–7.6 s variable). The plan expects "repaints at once". Placement may be memoised (test passes), but the warp to 4500×2108 still reruns for a palette-only change. Palette or range changes could repaint from cached values, or the warp's pixel→cell map could be cached with the placement.
§42, §43: no manual check (plan)
§45.1 priority regular_latlon Equirect Auto 270.5..311.1; small box 20–40N 5–20E and Bilinear: numbers unchanged: PASS
§45.2 Manual min 300, max cleared: bar 300..311.1, cells below 300 lowest colour: PASS
§45.3 300/290: picture kept; "minimum 300 is not below maximum 290; set a minimum below the maximum", both outlined: PASS
§45.4 min 400 max cleared: "minimum 400 is above the field's maximum 311.0986328125 (taken from the data); set a maximum too", only min outlined: PASS (cosmetic: max printed unrounded)
§45.5 300/290 + Export PNG: nothing saved, "Export failed: the colour range in the fields was refused; see the range fields."; status "Colour range not drawn: see the range fields": PASS. Compare-on-another-grid sub-step not run (needs a two-grid file open as this panel).
§45.6 min 280: draws 280..311.1, note/outline/export line gone: PASS
§45.7 priority gfs_wind manual −30..30 + contours; Compare A−B Field B VGRD: map + isolines follow the difference; Compare off: isolines pixel-identical to the first picture: PASS
§46.1 ncep A−B (A day 100, B day 10) line at 45N 10E along lon: "A − B · air across lon · −5.25 to 27.23 degK" = file; readout 7.1250 K: PASS
§46.2 along lat: column of the difference (−30.93..15.95): PASS
§46.3 priority along time: −3.10..24.25 (file), 7.125 at the day-100 rule, zero at day 10: PASS
§46.4 B → 200: 45N line −25.80..1.55 (file), day 100 −15.575 (plan −15.6), zero 07-19; A stepped to 300: same line, rule moved: PASS
§46.5 zonal both day 100 flat 0 ("0 degK · 73 of 73"); B 10: not flat, +ve N mid-lat; Compare off 219.38..301.13: PASS — C13
C13 (minor) After ticking Zonal average with Line plot ticked, both boxes show ticked (the line was replaced by the zonal plot). The plan says each replaces the other; the Line plot box should untick, as a click unticks Zonal (render-panel.ts offerLine).
§46.6 gfs_wind zonal off −5.00..9.15; A+B (B = UGRD) −9.99..18.31 (×2, last digit ±1): PASS
§46.7 A−B UGRD: flat "0 m s⁻¹"; Field B VGRD: −7.48..11.34: PASS
§44 (local pack.sh build at 957ef367, not the dry-run tarball — no dry run since 31bbe97e) headless Chrome 155: grib2 16x31 Temperature K valid=496 270.47..311.10, canvas matches extension source view north up; netcdf 260x200 kind=lookup label=curvilinear; t2m 7: 52 ms 1038240 −90..90, 8: 28 ms 0 null..null, 119: 40 ms 10..190; done; no console errors: PASS
§44 smoke (SwiftShader WebGL, no real GPU): "PASS — 151987 pixels compared, 0 off by one lookup entry, 0 mismatched.": PASS
Not run: §1 keyboard path to mirs line (mouse only); §45.5 Compare-on-other-grid sub-step.
