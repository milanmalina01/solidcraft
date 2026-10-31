//! A design's schematic can hold section frames (`ecad.schematic.frames`): each an `id` from the sheet's id space, a
//! `title` (no two frames' titles alike ignoring case), a `rect` (the lower left and upper right corners in sheet
//! millimetres) and an optional `order` (print order, and which frame wins where frames overlap). A print splits the
//! sheet into pages by them; nothing else reads them. A design's electronics can also hold the title block's fields
//! (`ecad.info`): `title`, `revision`, `date` (the user's text, never the clock), `author`, `company` and `notes`, each
//! left out while empty, and `revisions`, the revision history for the cover, oldest first: at most 50 of
//! `{rev, date?, by?, note?}` (`rev` not empty; the others left out while empty). Older designs need no
//! transformation: without `frames` the sheet has none, without `info` the title block falls back to the design's
//! properties, and without `revisions` the cover prints one row from the current revision, date and author. Empty
//! lists and an empty `info` are not saved. A build without this change refuses a design that has any of them, and the
//! last release refuses the newer format number.
