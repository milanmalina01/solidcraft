//! A board can keep markings (`ecad.board.markings`, from `ecad.board.marking`): each an `id`, the `side` (top or
//! bottom) its silkscreen block goes on, `at` (the block's top left), an optional `number` (the part number's
//! `{number}`), the `template` it was made from (`title`, `revision`, `date`, `part_number`, `serial_box`, `qr`,
//! `qr_text` (what the QR code holds when not the part number and revision; left out while empty), `made_by`,
//! `order_placeholder`, `height`) and optionally `layer_marks`. Its drawings (`graphics[]`) and the keepouts that
//! clear pours round its copper layer marks (`keepouts[]`) carry `marking`, the marking's id, so they are made
//! again together when the design's revision or date changes. Older designs need no transformation: without
//! `markings` a board has none, and without `marking` a drawing or keepout belongs to none. A build without this
//! change refuses a design that has any of them, and the last release refuses the newer format number.
