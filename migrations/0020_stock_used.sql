-- Use Stock: every time part of a stock item (a returned product kept as stock) is sent to a customer order.
-- The stock item itself is not changed: what is left is its QTY minus the rows here that are not voided.
CREATE TABLE IF NOT EXISTS stock_used (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `at` DATETIME NOT NULL,
  `operator` VARCHAR(100) NOT NULL DEFAULT '',
  `source` VARCHAR(10) NOT NULL COMMENT 'order = a row of All Orders with Stock ticked, kohls = a row of Kohl''s Returns',
  `ref` BIGINT UNSIGNED NOT NULL COMMENT 'the rn of the order, or the id of the Kohl''s row',
  `stock_no` VARCHAR(100) NOT NULL DEFAULT '',
  `sku` VARCHAR(100) NOT NULL DEFAULT '',
  `product` TEXT NULL,
  `qty` INT NOT NULL,
  `order_ref` VARCHAR(200) NOT NULL DEFAULT '' COMMENT 'the customer order it went to (Store Order No. or Label Number)',
  `note` TEXT NULL,
  `voided` TINYINT(1) NOT NULL DEFAULT 0,
  `voided_by` VARCHAR(100) NOT NULL DEFAULT '',
  `voided_at` DATETIME NULL,
  PRIMARY KEY (`id`),
  KEY `ix_stock_used_item` (`source`, `ref`),
  KEY `ix_stock_used_at` (`at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
