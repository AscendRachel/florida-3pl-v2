-- The Additional Task and SKU lists are columns of the check list table: each line sits in a row (its position), so it stays where it was typed.
ALTER TABLE checklist_items ADD COLUMN `pos` INT NOT NULL DEFAULT 0 AFTER `kind`;
UPDATE checklist_items SET pos = id;
ALTER TABLE checklist_items ADD UNIQUE KEY `uq_ci_cell` (`kind`, `pos`);
