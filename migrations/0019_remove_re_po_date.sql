-- The RE-P.O Date column is not wanted: the RE-P.O card counts the RE-P.O rows that are still Preshipment, right now.
-- It is the last column, so no other column number moves.
DELETE FROM column_defs WHERE `field` = 're_po_date';
ALTER TABLE orders DROP COLUMN `re_po_date`;
