//! Section frames on the schematic: named rectangles a print splits the sheet into pages by
//! (`ecad.export.pages`). Made and changed here, each call one undo step; moved, deleted and read
//! back by the sheet's own commands (`ecad.schematic.move`, `.delete`, `.inspect`).

use serde_json::Value;
use solidcraft_doc::ecad::frame::{MAX_FRAMES, check_title, in_order, normalized, overlaps, part_frames};
use solidcraft_doc::ecad::schematic::check_sheet_point;
use solidcraft_doc::ecad::{Frame, Library, Schematic};

use super::CommandSpec;
use super::ecad::{closest, parse, reply, store_schematic};
use crate::params::bad;
use crate::schema::ecad_frame::{FrameEditParams, FrameOut, FrameParams, FrameRef, FrameResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.schematic.frame", "Frame", crate::checked!("ecad.schematic.frame", frame))
        .icon("rect_rounded")
        .doc_only()
        .contract(&FRAME_CONTRACT)
        .output(output_schema_for::<FrameResult>)
        .params(
            "title (1 to 64 characters, unique among the frames), rect: [[x0, y0], [x1, y1]] (two opposite corners, sheet mm), order? (print order, lower first) → {frame: {id, title, rect, order?, parts: [designators]}, frames, outside: [designators], overlaps?}. A part belongs to the frame holding its symbol's middle (the first in print order where frames overlap). Move and delete frames with ecad.schematic.move / .delete; ecad.schematic.inspect lists them",
        ),
    CommandSpec::new("ecad.schematic.frame.edit", "Edit Frame", crate::checked!("ecad.schematic.frame.edit", edit))
        .icon("edit")
        .doc_only()
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<FrameResult>)
        .params("frame: id|title, title?, rect? [[x0, y0], [x1, y1]], order? (null: none) → as ecad.schematic.frame"),
];

const FRAME_CONTRACT: TypedContract<FrameParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<FrameEditParams> = TypedContract::new();

/// The frames as replies give them, in print order, with the parts each holds.
pub(super) fn frames_out(sheet: &Schematic, library: &Library) -> Vec<FrameOut> {
    let members = part_frames(sheet, library);
    in_order(&sheet.frames)
        .into_iter()
        .map(|f| {
            let mut parts: Vec<String> = sheet.parts.iter().filter(|p| members.get(&p.id) == Some(&f.id)).map(|p| p.designator.clone()).collect();
            parts.sort_by(|a, b| solidcraft_doc::ecad::connect::natural_cmp(a, b));
            FrameOut { id: f.id, title: f.title.clone(), rect: f.rect, order: f.order, parts }
        })
        .collect()
}

fn result(s: &Session, id: u64) -> Result<Value> {
    let sheet = &s.doc.ecad.schematic;
    let library = s.doc.ecad.library();
    let frames = frames_out(sheet, &library);
    let members = part_frames(sheet, &library);
    let mut outside: Vec<String> = sheet.parts.iter().filter(|p| !members.contains_key(&p.id)).map(|p| p.designator.clone()).collect();
    outside.sort_by(|a, b| solidcraft_doc::ecad::connect::natural_cmp(a, b));
    let order = in_order(&sheet.frames);
    let title = |id: u64| sheet.frames.iter().find(|f| f.id == id).map_or(String::new(), |f| f.title.clone());
    let overlaps = overlaps(&sheet.frames)
        .into_iter()
        .map(|(a, b)| {
            let first = order.iter().find(|f| f.id == a || f.id == b).map_or(a, |f| f.id);
            format!("`{}` and `{}` overlap: parts in both belong to `{}`", title(a), title(b), title(first))
        })
        .collect();
    let Some(frame) = frames.iter().find(|f| f.id == id).cloned() else {
        return Err(crate::EngineError::Other(format!("frame {id} is not on the sheet")));
    };
    reply(FrameResult { frame, frames, outside, overlaps, param_warnings: Vec::new() })
}

/// A frame's corners: finite, on the sheet, lower left first, sides long enough.
fn rect(r: [[f64; 2]; 2], cmd: &str) -> Result<[[f64; 2]; 2]> {
    for p in r {
        check_sheet_point("`rect`", p).map_err(|e| bad(cmd, e))?;
    }
    Ok(normalized(r))
}

/// A frame title no other frame (but `except`) has, ignoring case.
fn title(sheet: &Schematic, t: &str, except: Option<u64>, cmd: &str) -> Result<String> {
    check_title(t).map_err(|e| bad(cmd, e))?;
    if let Some(other) = sheet.frames.iter().find(|f| Some(f.id) != except && f.title.eq_ignore_ascii_case(t)) {
        return Err(bad(cmd, format!("a frame is already titled `{}`: frame titles differ in more than case", other.title)));
    }
    Ok(t.to_string())
}

fn frame(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.frame";
    let p: FrameParams = parse(CMD, p)?;
    let mut sheet = s.doc.ecad.schematic.clone();
    if sheet.frames.len() >= MAX_FRAMES {
        return Err(bad(CMD, format!("a schematic holds at most {MAX_FRAMES} frames")));
    }
    let f = Frame { id: 0, title: title(&sheet, &p.title, None, CMD)?, rect: rect(p.rect, CMD)?, order: p.order };
    f.validate().map_err(|e| bad(CMD, e))?;
    let id = sheet.take_id().map_err(|e| bad(CMD, e))?;
    sheet.frames.push(Frame { id, ..f });
    store_schematic(s, sheet, CMD)?;
    result(s, id)
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.frame.edit";
    let p: FrameEditParams = parse(CMD, p)?;
    let mut sheet = s.doc.ecad.schematic.clone();
    let id = match &p.frame {
        FrameRef::Id(id) => sheet.frames.iter().find(|f| f.id == *id).map(|f| f.id),
        FrameRef::Title(t) => {
            sheet.frames.iter().find(|f| f.title == *t).or_else(|| sheet.frames.iter().find(|f| f.title.eq_ignore_ascii_case(t))).map(|f| f.id)
        }
    };
    let Some(id) = id else {
        let mut msg = "`frame`: the schematic has no such frame".to_string();
        if let FrameRef::Title(t) = &p.frame
            && let Some(near) = closest(t, sheet.frames.iter().map(|f| f.title.as_str()))
        {
            msg.push_str(&format!(" (did you mean `{near}`?)"));
        } else if sheet.frames.is_empty() {
            msg.push_str(" (it has none yet: make one with ecad.schematic.frame)");
        }
        return Err(bad(CMD, msg));
    };
    let new_title = p.title.as_deref().map(|t| title(&sheet, t, Some(id), CMD)).transpose()?;
    let new_rect = p.rect.map(|r| rect(r, CMD)).transpose()?;
    let Some(f) = sheet.frames.iter_mut().find(|f| f.id == id) else { return Err(bad(CMD, "`frame`: the schematic has no such frame")) };
    if let Some(t) = new_title {
        f.title = t;
    }
    if let Some(r) = new_rect {
        f.rect = r;
    }
    if let Some(o) = p.order {
        f.order = o;
    }
    f.validate().map_err(|e| bad(CMD, e))?;
    store_schematic(s, sheet, CMD)?;
    result(s, id)
}
