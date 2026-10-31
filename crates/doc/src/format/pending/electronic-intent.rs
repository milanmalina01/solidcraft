//! A design's electronics can hold the design's intent (`ecad.intent`): what the netlist alone doesn't say. `board`
//! (facts about the whole board, each left out until known: `product`, prototype or production, `ipc_class` 2 or 3,
//! `enclosure` none, plastic or metal, `f_concern_mhz`, `bands`, the radio bands it works in, each a `name`, `lo_mhz`,
//! `hi_mhz` and `rx`, `hand_assembly`, and `by` and `note`), `nets` (each a `net` by name, a `role`: power, ground, rf,
//! diff, clock, strobe, fast, interface, analog, sensitive, slow, static, hv, sw_node, fb or sense, and optionally
//! `current_a`, `voltage` with a `nominal` and a `peak`, `edge_ns`, `driver`, `z_target`, `z_tol`, `interface`,
//! `f_mhz`, `external` and `domain`), `pairs` (a `name`, the `p` and `n` nets, optionally `interface` and `skew_mm`),
//! `parts` (a `part` by designator, a `role` such as decoupling, crystal, tvs, connector, smps_ic or power_ic (a
//! charger, power-path IC, power switch or linear regulator), optionally `of`, the pins, parts or nets it serves,
//! `cable` for a connector, `power_w` and `loop`), `loops` (a `name`, a `kind`:
//! buck_hot, boost_hot, gate, decap, crystal, tvs or sense, its `members` as pins and optionally `reference_ratio`) and
//! `domains` (a `name`, a `kind` and its `parts`). Each net, pair, part, loop and domain record has a `source`
//! (declared, inferred or confirmed; declared when left out), and optionally `by` (user or agent), `confidence`,
//! `evidence` and a `note`. Records name nets and parts as the schematic shows them; a name it doesn't have is kept.
//! Older designs need no transformation: without `intent` nothing is declared, and an empty intent is not saved. A
//! build without this change refuses a design that has one, and the last release refuses the newer format number.
