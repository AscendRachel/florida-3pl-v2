-- All Orders: Source Delivery Days = Delivery Date minus Source Order Date, in days. The database works it out by itself (a computed column),
-- so it is always right after an edit, an import or an Amazon import, and nobody types into it. The index makes the Dashboard count quick.
ALTER TABLE orders
  ADD COLUMN `source_delivery_days` INT GENERATED ALWAYS AS (DATEDIFF(`delivery_date`, `source_order_date`)) VIRTUAL,
  ADD INDEX `ix_source_delivery_days` (`source_delivery_days`);

-- the column in the list of All Orders columns: at the end (the numbers of the other columns do not move)
INSERT INTO column_defs (`field`, `header`, `label`, `kind`, `position`, `custom`, `deleted`)
SELECT 'source_delivery_days', 'Source Delivery Days', 'Source Delivery Days', 'num', (SELECT IFNULL(MAX(position), 0) + 1 FROM column_defs), 0, 0
WHERE NOT EXISTS (SELECT 1 FROM column_defs WHERE `field` = 'source_delivery_days');
