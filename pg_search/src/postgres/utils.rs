// Copyright (c) 2023-2024 Retake, Inc.
//
// This file is part of ParadeDB - Postgres for Search and Analytics
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <http://www.gnu.org/licenses/>.

use crate::postgres::types::TantivyValue;
use crate::schema::{SearchDocument, SearchFieldName, SearchIndexSchema};
use crate::writer::IndexError;
use pgrx::pg_sys::{BuiltinOid, ItemPointerData};
use pgrx::*;

pub unsafe fn row_to_search_document(
    ctid: ItemPointerData,
    tupdesc: &PgTupleDesc,
    values: *mut pg_sys::Datum,
    isnull: *mut bool,
    schema: &SearchIndexSchema,
) -> Result<SearchDocument, IndexError> {
    let mut document = schema.new_document();

    // Create a vector of index entries from the postgres row.
    for (attno, attribute) in tupdesc.iter().enumerate() {
        let attname = attribute.name().to_string();
        let attribute_type_oid = attribute.type_oid();

        // If we can't lookup the attribute name in the field_lookup parameter,
        // it means that this field is not part of the index. We should skip it.
        let search_field =
            if let Some(index_field) = schema.get_search_field(&attname.clone().into()) {
                index_field
            } else {
                continue;
            };

        let array_type = unsafe { pg_sys::get_element_type(attribute_type_oid.value()) };
        let (base_oid, is_array) = if array_type != pg_sys::InvalidOid {
            (PgOid::from(array_type), true)
        } else {
            (attribute_type_oid, false)
        };

        let is_json = matches!(
            base_oid,
            PgOid::BuiltIn(BuiltinOid::JSONBOID | BuiltinOid::JSONOID)
        );

        let datum = *values.add(attno);
        let isnull = *isnull.add(attno);

        let SearchFieldName(key_field_name) = schema.key_field().name;
        if key_field_name == attname && isnull {
            return Err(IndexError::KeyIdNull(key_field_name));
        }

        if isnull {
            continue;
        }

        if is_array {
            for value in TantivyValue::try_from_datum_array(datum, base_oid)? {
                document.insert(search_field.id, value.tantivy_schema_value());
            }
        } else if is_json {
            for value in TantivyValue::try_from_datum_json(datum, base_oid)? {
                document.insert(search_field.id, value.tantivy_schema_value());
            }
        } else {
            document.insert(
                search_field.id,
                TantivyValue::try_from_datum(datum, base_oid)?.tantivy_schema_value(),
            );
        }
    }

    // Insert the ctid value into the entries.
    let ctid_index_value = pgrx::item_pointer_to_u64(ctid);
    document.insert(schema.ctid_field().id, ctid_index_value.into());

    Ok(document)
}

pub unsafe fn ctid_satisfies_snapshot(
    ctid: u64,
    relation: pg_sys::Relation,
    snapshot: pg_sys::Snapshot,
) -> bool {
    // Using ctid, get itempointer => buffer => page => heaptuple
    let mut item_pointer = pg_sys::ItemPointerData::default();
    pgrx::u64_to_item_pointer(ctid, &mut item_pointer);

    let blockno = item_pointer_get_block_number(&item_pointer);
    let offsetno = item_pointer_get_offset_number(&item_pointer);
    let buffer = pg_sys::ReadBuffer(relation, blockno);
    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);

    let page = pg_sys::BufferGetPage(buffer);
    let item_id = pg_sys::PageGetItemId(page, offsetno);
    let mut heap_tuple = pg_sys::HeapTupleData {
        t_data: pg_sys::PageGetItem(page, item_id) as pg_sys::HeapTupleHeader,
        t_len: item_id.as_ref().unwrap().lp_len(),
        t_tableOid: (*relation).rd_id,
        t_self: item_pointer,
    };

    // Check if heaptuple is visible
    // In Postgres, the indexam `amgettuple` calls `heap_hot_search_buffer` for its visibility check
    let visible = pg_sys::heap_hot_search_buffer(
        &mut item_pointer,
        relation,
        buffer,
        snapshot,
        &mut heap_tuple,
        std::ptr::null_mut(),
        true,
    );
    pg_sys::UnlockReleaseBuffer(buffer);

    visible
}
