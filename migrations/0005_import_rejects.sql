-- Values of the old data that did not fit the new column type (for example text in a date column). The import stores NULL in the
-- orders table and keeps the original text here, so nothing the old app held is lost.
CREATE TABLE import_rejects (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `source_table` VARCHAR(64) NOT NULL,
  `row_ref` VARCHAR(64) NOT NULL COMMENT 'the order number (rn) or row id in the old data',
  `field` VARCHAR(64) NOT NULL,
  `value` TEXT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_rejects_row` (`source_table`, `row_ref`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
