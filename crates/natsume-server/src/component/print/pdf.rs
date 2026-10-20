//! lopdf 0.45 strict syntax/object parsing plus a passive A4 admission profile.
//!
//! This is not a renderer: content operators, embedded fonts, image codecs and
//! printer compatibility are not fully interpreted. Parsing and decompression
//! run in-process, without a memory or execution-time isolation guarantee.
use std::collections::HashSet;

use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId};

use super::PrintError;

type Result<T> = std::result::Result<T, PrintError>;

pub(super) fn validate(bytes: &[u8]) -> Result<i64> {
    let doc = Document::load_mem_with_options(
        bytes,
        LoadOptions {
            strict: true,
            ..LoadOptions::default()
        },
    )
    .map_err(|_| PrintError::Invalid)?;
    if doc.trailer.has(b"Encrypt") || doc.is_encrypted() || doc.encryption_state.is_some() {
        return Err(PrintError::Invalid);
    }
    validate_objects(&doc)?;
    let catalog = doc.catalog().map_err(|_| PrintError::Invalid)?;
    if !catalog.has_type(b"Catalog") {
        return Err(PrintError::Invalid);
    }
    let root = catalog
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|_| PrintError::Invalid)?;
    page_tree(&doc, root)
}

fn validate_xref(doc: &Document) -> Result<()> {
    for (&number, entry) in &doc.reference_table.entries {
        let id = match entry {
            lopdf::xref::XrefEntry::Normal { generation, .. } => (number, *generation),
            lopdf::xref::XrefEntry::Compressed { .. } => (number, 0),
            lopdf::xref::XrefEntry::Free | lopdf::xref::XrefEntry::UnusableFree => continue,
        };
        if !doc.objects.contains_key(&id) {
            return Err(PrintError::Invalid);
        }
    }
    Ok(())
}

fn validate_objects(doc: &Document) -> Result<()> {
    validate_xref(doc)?;
    let mut pending: Vec<&Object> = doc.objects.values().collect();
    pending.extend(doc.trailer.iter().map(|(_, v)| v));
    while let Some(object) = pending.pop() {
        let dictionary = match object {
            Object::Reference(id) => {
                if !doc.objects.contains_key(id) {
                    return Err(PrintError::Invalid);
                }
                continue;
            }
            Object::Array(values) => {
                pending.extend(values);
                continue;
            }
            Object::Dictionary(dict) => dict,
            Object::Stream(stream) => {
                let length = resolve(
                    doc,
                    stream
                        .dict
                        .get(b"Length")
                        .map_err(|_| PrintError::Invalid)?,
                )?
                .as_i64()
                .map_err(|_| PrintError::Invalid)?;
                if usize::try_from(length).ok() != Some(stream.content.len()) {
                    return Err(PrintError::Invalid);
                }
                // /F makes a stream read its data from an external file.
                if [b"F".as_slice(), b"FFilter", b"FDecodeParms"]
                    .iter()
                    .any(|key| stream.dict.has(key))
                {
                    return Err(PrintError::Invalid);
                }
                &stream.dict
            }
            _ => continue,
        };
        for (key, value) in dictionary {
            if forbidden_key(key) {
                return Err(PrintError::Invalid);
            }
            let resolved = resolve(doc, value)?;
            if key == b"Type"
                && matches!(
                    resolved.as_name(),
                    Ok(b"Action" | b"Filespec" | b"EmbeddedFile" | b"RichMedia" | b"Rendition")
                )
            {
                return Err(PrintError::Invalid);
            }
            if key == b"Subtype" && matches!(resolved.as_name(), Ok(b"PS" | b"3D" | b"RichMedia")) {
                return Err(PrintError::Invalid);
            }
            if key == b"Filter" {
                let filters = match resolved {
                    Object::Array(values) => values.as_slice(),
                    value => std::slice::from_ref(value),
                };
                for filter in filters {
                    if matches!(resolve(doc, filter)?.as_name(), Ok(b"Crypt")) {
                        return Err(PrintError::Invalid);
                    }
                }
            }
            if key == b"S"
                && matches!(
                    resolved.as_name(),
                    Ok(b"GoTo"
                        | b"GoToR"
                        | b"GoToE"
                        | b"Launch"
                        | b"Thread"
                        | b"URI"
                        | b"Sound"
                        | b"Movie"
                        | b"Hide"
                        | b"Named"
                        | b"SubmitForm"
                        | b"ResetForm"
                        | b"ImportData"
                        | b"JavaScript"
                        | b"SetOCGState"
                        | b"Rendition"
                        | b"Trans"
                        | b"GoTo3DView")
                )
            {
                return Err(PrintError::Invalid);
            }
            pending.push(value);
        }
    }
    Ok(())
}

fn forbidden_key(key: &[u8]) -> bool {
    matches!(
        key,
        b"F" | b"FS"
            | b"FDecodeParms"
            | b"FFilter"
            | b"3DD"
            | b"3DV"
            | b"Movie"
            | b"Sound"
            | b"A"
            | b"AA"
            | b"OpenAction"
            | b"JS"
            | b"JavaScript"
            | b"URI"
            | b"EF"
            | b"RF"
            | b"AF"
            | b"EmbeddedFiles"
            | b"AcroForm"
            | b"XFA"
            | b"Annots"
            | b"RichMediaContent"
            | b"RichMediaSettings"
            | b"Collection"
            | b"OPI"
            | b"Ref"
            | b"Alternates"
            | b"URL"
    )
}

fn resolve<'a>(doc: &'a Document, mut object: &'a Object) -> Result<&'a Object> {
    let mut seen = HashSet::new();
    while let Object::Reference(id) = object {
        if !seen.insert(*id) {
            return Err(PrintError::Invalid);
        }
        object = doc.objects.get(id).ok_or(PrintError::Invalid)?;
    }
    Ok(object)
}

#[derive(Clone, Default)]
struct PageBoxes {
    media: Option<Vec<f32>>,
    crop: Option<Vec<f32>>,
}

enum Visit {
    Node(ObjectId, Option<ObjectId>, PageBoxes),
    End(i64, i64),
}

fn page_tree(doc: &Document, root: ObjectId) -> Result<i64> {
    let mut pending = vec![Visit::Node(root, None, PageBoxes::default())];
    let mut seen = HashSet::new();
    let mut pages = 0_i64;
    while let Some(visit) = pending.pop() {
        let (id, parent, mut boxes) = match visit {
            Visit::End(start, count) => {
                if pages - start != count {
                    return Err(PrintError::Invalid);
                }
                continue;
            }
            Visit::Node(id, parent, boxes) => (id, parent, boxes),
        };
        if !seen.insert(id) {
            return Err(PrintError::Invalid);
        }
        let dict = doc.get_dictionary(id).map_err(|_| PrintError::Invalid)?;
        match parent {
            Some(parent)
                if dict.get(b"Parent").and_then(Object::as_reference).ok() != Some(parent) =>
            {
                return Err(PrintError::Invalid);
            }
            None if dict.has(b"Parent") => return Err(PrintError::Invalid),
            _ => {}
        }
        for (key, dest) in [
            (b"MediaBox".as_slice(), &mut boxes.media),
            (b"CropBox".as_slice(), &mut boxes.crop),
        ] {
            if let Ok(value) = dict.get(key) {
                *dest = Some(rectangle(doc, value)?);
            }
        }
        if let Ok(rotation) = dict.get(b"Rotate")
            && resolve(doc, rotation)?
                .as_i64()
                .map_err(|_| PrintError::Invalid)?
                % 90
                != 0
        {
            return Err(PrintError::Invalid);
        }
        if dict.has_type(b"Pages") {
            let kids = resolve(doc, dict.get(b"Kids").map_err(|_| PrintError::Invalid)?)?
                .as_array()
                .map_err(|_| PrintError::Invalid)?;
            let count = resolve(doc, dict.get(b"Count").map_err(|_| PrintError::Invalid)?)?
                .as_i64()
                .map_err(|_| PrintError::Invalid)?;
            if count < 0 || kids.is_empty() {
                return Err(PrintError::Invalid);
            }
            pending.push(Visit::End(pages, count));
            for kid in kids.iter().rev() {
                pending.push(Visit::Node(
                    kid.as_reference().map_err(|_| PrintError::Invalid)?,
                    Some(id),
                    boxes.clone(),
                ));
            }
        } else if dict.has_type(b"Page") {
            if dict.has(b"Kids") || dict.has(b"Count") {
                return Err(PrintError::Invalid);
            }
            if let Ok(unit) = dict.get(b"UserUnit")
                && (number(resolve(doc, unit)?)? - 1.0).abs() > f32::EPSILON
            {
                return Err(PrintError::Invalid);
            }
            let media = boxes.media.ok_or(PrintError::Invalid)?;
            if !is_a4(&media)
                || boxes
                    .crop
                    .as_ref()
                    .is_some_and(|crop| !is_a4(crop) || !contains(&media, crop))
            {
                return Err(PrintError::Invalid);
            }
            validate_contents(doc, dict)?;
            pages = pages.checked_add(1).ok_or(PrintError::Invalid)?;
        } else {
            return Err(PrintError::Invalid);
        }
    }
    if pages == 0 {
        return Err(PrintError::Invalid);
    }
    Ok(pages)
}

fn validate_contents(doc: &Document, page: &Dictionary) -> Result<()> {
    if let Ok(contents) = page.get(b"Contents") {
        match resolve(doc, contents)? {
            Object::Stream(stream) => validate_content_stream(stream)?,
            Object::Array(parts) => {
                for part in parts {
                    let Object::Stream(stream) = resolve(doc, part)? else {
                        return Err(PrintError::Invalid);
                    };
                    validate_content_stream(stream)?;
                }
            }
            _ => return Err(PrintError::Invalid),
        }
    }
    Ok(())
}

fn validate_content_stream(stream: &lopdf::Stream) -> Result<()> {
    let content = if stream.dict.has(b"Filter") {
        stream
            .decompressed_content()
            .map_err(|_| PrintError::Invalid)?
    } else {
        stream.content.clone()
    };
    lopdf::content::Content::decode(&content).map_err(|_| PrintError::Invalid)?;
    Ok(())
}

fn number(value: &Object) -> Result<f32> {
    let n = value.as_float().map_err(|_| PrintError::Invalid)?;
    if !n.is_finite() {
        return Err(PrintError::Invalid);
    }
    Ok(n)
}

fn rectangle(doc: &Document, value: &Object) -> Result<Vec<f32>> {
    let values = resolve(doc, value)?
        .as_array()
        .map_err(|_| PrintError::Invalid)?;
    if values.len() != 4 {
        return Err(PrintError::Invalid);
    }
    values.iter().map(|v| number(resolve(doc, v)?)).collect()
}

fn is_a4(rect: &[f32]) -> bool {
    let w = rect[2] - rect[0];
    let h = rect[3] - rect[1];
    // PDF points round the exact millimetre dimensions; allow 0.1 point rounding.
    ((w - 595.2756).abs() <= 0.1 && (h - 841.8898).abs() <= 0.1)
        || ((h - 595.2756).abs() <= 0.1 && (w - 841.8898).abs() <= 0.1)
}

fn contains(outer: &[f32], inner: &[f32]) -> bool {
    inner[0] >= outer[0] && inner[1] >= outer[1] && inner[2] <= outer[2] && inner[3] <= outer[3]
}
