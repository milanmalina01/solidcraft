//! A design's electronics can hold a circuit board (`ecad.board`): an `outline` (a closed polygon of corners `[x, y]`
//! in millimetres; none while empty) with `outline_curves` (one sweep in degrees per edge, the closing edge last,
//! counter-clockwise when positive and 0 for a straight edge; left out while every edge is straight), a `thickness`
//! (1.6 mm when left out), the number of `copper_layers` (even, 2 to 16; 2 when left out), `footprints` (each an `id`,
//! the schematic `part` it is for, its origin `at`, a `rotation` of 0, 90, 180 or 270 degrees, a `side`, top or bottom,
//! a bottom one mirrored, and optionally `name_at` and `value_at`, the offsets of its name and value texts from `at`,
//! `show_name` false to hide the name, `show_value` true to show the value, `pour_connect` (`thermal`, `solid` or
//! `none`: how pours of its pads' nets join its pads, over each pour's own `connect`) and `pad_connect`, the same per
//! pad by pad number, over `pour_connect`; a number its footprint lacks, after the part's footprint changed, is kept
//! and applies again to a footprint with that pad), `tracks` (an `id`, a copper `layer`:
//! `top`, `inner1` … `inner14` within the stack, or `bottom`, a `width`, two or more `points` and optionally `curves`,
//! one sweep per segment as for the outline), `vias` (an `id`, `at`, a `diameter`, a `drill`, and optionally `from` and
//! `to`, the copper layers of its span, `from` above `to`, `top` and `bottom` when left out, `net`, a schematic
//! net's name it is on of its own, `protection`: open when left out, tented, plugged, filled or filled_capped (a
//! through or blind via sits in a pad only filled_capped; a laser micro via may as it is, filled when stacked), and
//! `role`: `stitch` for a stitching via, placed with others to tie a net's copper together and removed with them,
//! left out for any other, and for a stitching via its `pattern`, `grid`, `fence` or `layer_change`, so a removal can
//! take one pattern's alone; left out on one stitched before patterns were kept, which a removal of one pattern
//! leaves unless asked to take it too), `holes`
//! (unplated mounting holes: an `id`, `at` and a `drill`), `graphics` (board
//! drawings: an `id`, a `layer` that is `silkscreen_top`, `silkscreen_bottom`, `documentation` or a copper layer,
//! `mirror` true for mirrored text, and the `graphic` itself, shaped as a footprint graphic without its `layer` and
//! `side`, its points and radius within the board's 2000 mm rather than 1000 mm), `pours` (copper pours: an `id`, a
//! copper `layer`, optionally a `net` by name, an `outline` of corners with `curves` as for the board outline, and
//! optionally `clearance`, `min_width` (0.2 when left out), `connect` (`thermal` when left out, `solid` or `none`),
//! `thermal` with `spoke_width`, `gap` and `spokes` (0.3, 0.3 and 4 when left out), `priority` (0 when left out),
//! `remove_islands` (true when left out) and `hatch` with a `width` and a `pitch`; the fill is worked out, never
//! saved), `keepouts` (an `id`, the copper `layers` it covers, every layer when left out, an `outline` with `curves`,
//! and `forbid` with `tracks`, `vias`, `pours` and `components`, each true when left out), the `stackup` (top first:
//! optional `mask`, then `copper` and `core` or `prepreg` in turn, each with a `thickness` and, for a dielectric or the
//! mask, optionally `er` and `loss_tangent`; one copper entry per copper layer and within 10 % of the thickness; the
//! default stack-up when left out), the design `rules` (`track_width`, `clearance`, `via_diameter`, `via_drill`,
//! `min_track_width`, `min_annular_ring`, `min_drill`, `hole_to_hole`, `copper_to_edge`, `silk_to_pad`,
//! `microvia_diameter`, `microvia_drill` and `net_classes`, each a `name`, its `nets` by name, a `track_width`,
//! `clearance`, `via_diameter`, `via_drill` and optionally a `diff_pair` with a `width` and a `gap`, the `current` its
//! nets carry in amperes, the `temp_rise` allowed at it in °C (10 when left out) and `copper_oz`, the copper's
//! thickness in oz/ft² the width is for (the stack-up's when left out); the defaults when left out), `name_height`
//! (the height in millimetres of every footprint's name and value text, 0.1 to 10; left out, each footprint's text is
//! sized to its courtyard, 0.5 to 1 mm, or as its library footprint's spot for it says), the design rule check's
//! `approvals` (each a `key`, optionally a `note`, and `by`, user or agent, who approved it; left out, not known, and a
//! critical finding's then waits for the user's confirmation as an agent's does),
//! `fab_profile` (the fab profile it is checked against: an `id`, standard, fine or hdi, and the `version` of its
//! figures when it was picked; a board saved without one has none chosen, a new board gets the standard one),
//! `drc_levels` (a level per design rule check code: error, warning, info or ignore), the design-quality checks'
//! `quality_approvals` (shaped as `approvals`), `quality_levels` (a level per quality code, such as
//! `pi.return_path`) and `quality_params` (by `code.param`, such as `pi.layer_change.reach`: a `value` within the
//! parameter's range, optionally a `note` saying why and `by`, user or agent), each left out while empty, and the id
//! counter `next_id`.
//! The rules' defaults are the standard profile's minimums: a 0.15 mm annular ring, 0.3 mm drill and 0.15 mm
//! silkscreen-to-pad gap.
//! A footprint's pads and graphics are those of its part's variant's footprint; pad nets and airwires follow from the
//! schematic and are not saved. Older designs need no transformation: without `board` the board is empty, and an empty
//! board is not saved. A build without this change refuses a design that has one, and the last release refuses the
//! newer format number.
