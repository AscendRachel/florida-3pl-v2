-- A column has a fixed header (what the program knows it by) and a label (what an admin may rename it to).
-- The kind also covers the pages' 'img' and 'link' kinds, so it is plain text, not a short list.
ALTER TABLE column_defs MODIFY `kind` VARCHAR(16) NOT NULL DEFAULT 'text';
ALTER TABLE column_defs ADD COLUMN `header` VARCHAR(255) NOT NULL DEFAULT '' AFTER `field`;
