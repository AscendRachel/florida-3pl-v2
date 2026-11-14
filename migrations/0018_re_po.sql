-- All Orders: a RE-P.O checkbox and the RE-P.O Date, the date and time the box was ticked (the app stamps it, like the Hold Date).
-- The index makes the Dashboard's RE-P.O card (RE-P.Os ticked on a day) quick.
ALTER TABLE orders
  ADD COLUMN `re_po` TINYINT(1) NULL,
  ADD COLUMN `re_po_date` DATETIME NULL,
  ADD INDEX `ix_re_po_date` (`re_po_date`);

-- the two columns in the list of All Orders columns: at the end (the numbers of the other columns do not move)
INSERT INTO column_defs (`field`, `header`, `label`, `kind`, `position`, `custom`, `deleted`)
SELECT 're_po', 'RE-P.O', 'RE-P.O', 'bool', (SELECT IFNULL(MAX(position), 0) + 1 FROM column_defs), 0, 0
WHERE NOT EXISTS (SELECT 1 FROM column_defs WHERE `field` = 're_po');
INSERT INTO column_defs (`field`, `header`, `label`, `kind`, `position`, `custom`, `deleted`)
SELECT 're_po_date', 'RE-P.O Date', 'RE-P.O Date', 'date', (SELECT IFNULL(MAX(position), 0) + 1 FROM column_defs), 0, 0
WHERE NOT EXISTS (SELECT 1 FROM column_defs WHERE `field` = 're_po_date');
