-- The Returns, Kohl's and Materials tabs and the Stock Sorting counter.
-- (The old app kept most of these values as text; numbers and the TRUE/FALSE ticks are real types here.)

CREATE TABLE returns (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `rid` VARCHAR(64) NOT NULL,
  `return_key` VARCHAR(700) NOT NULL COMMENT 'what identifies the same return in the Google Sheet it is synced from',
  `store` VARCHAR(100) NOT NULL DEFAULT '',
  `po` VARCHAR(100) NOT NULL DEFAULT '',
  `product` TEXT NULL,
  `qty` INT NULL,
  `customer` VARCHAR(255) NOT NULL DEFAULT '',
  `address` TEXT NULL,
  `return_date` DATE NULL,
  `tracking` VARCHAR(1000) NOT NULL DEFAULT '',
  `status_cx` VARCHAR(100) NOT NULL DEFAULT '',
  `label_link` TEXT NULL,
  `due_date` DATE NULL,
  `notes` TEXT NULL,
  `status_wh` VARCHAR(100) NOT NULL DEFAULT '',
  `refunded` VARCHAR(100) NOT NULL DEFAULT '',
  `reason` VARCHAR(255) NOT NULL DEFAULT '',
  `shipped` TINYINT(1) NOT NULL DEFAULT 0,
  `processed_at` DATETIME NULL,
  `processed_by` VARCHAR(100) NOT NULL DEFAULT '',
  `wh_notes` TEXT NULL,
  `removed` TINYINT(1) NOT NULL DEFAULT 0,
  `first_seen` DATETIME NOT NULL,
  `last_seen` DATETIME NOT NULL,
  `ovr` JSON NULL COMMENT 'cells edited by hand, which a later sync must not overwrite',
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_returns_key` (`return_key`),
  KEY `ix_returns_po` (`po`),
  KEY `ix_returns_due` (`due_date`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE returns_sync_log (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `at` DATETIME NOT NULL,
  `ok` TINYINT(1) NOT NULL,
  `trigger_by` VARCHAR(40) NOT NULL DEFAULT '',
  `message` TEXT NULL,
  `read_count` INT NOT NULL DEFAULT 0,
  `added` INT NOT NULL DEFAULT 0,
  `updated` INT NOT NULL DEFAULT 0,
  `removed` INT NOT NULL DEFAULT 0,
  PRIMARY KEY (`id`),
  KEY `ix_rsl_at` (`at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE kohls (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `store` VARCHAR(100) NOT NULL DEFAULT '',
  `stock_no` VARCHAR(100) NOT NULL DEFAULT '',
  `sku` VARCHAR(100) NOT NULL DEFAULT '',
  `order_number` VARCHAR(100) NOT NULL DEFAULT '',
  `product` TEXT NULL,
  `image` TEXT NULL,
  `qty` INT NULL,
  `good` TINYINT(1) NOT NULL DEFAULT 0,
  `used` TINYINT(1) NOT NULL DEFAULT 0,
  `notes` TEXT NULL,
  `updated_by` VARCHAR(100) NOT NULL DEFAULT '',
  `updated_at` DATETIME NULL,
  PRIMARY KEY (`id`),
  KEY `ix_kohls_stock` (`stock_no`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE materials (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `mid` VARCHAR(64) NOT NULL DEFAULT '',
  `image` TEXT NULL,
  `name` VARCHAR(255) NOT NULL DEFAULT '',
  `spec` VARCHAR(255) NOT NULL DEFAULT '',
  `unit` VARCHAR(64) NOT NULL DEFAULT '',
  `pack` VARCHAR(64) NOT NULL DEFAULT '',
  `ordered` DECIMAL(12,2) NULL,
  `current` DECIMAL(12,2) NULL,
  `alert` DECIMAL(12,2) NULL,
  `minimum` DECIMAL(12,2) NULL,
  `status` VARCHAR(64) NOT NULL DEFAULT '',
  `updated_at` DATETIME NULL,
  `updated_by` VARCHAR(100) NOT NULL DEFAULT '',
  `notes` TEXT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- One row each time an operator puts a Stock No. on an item (the Dashboard's "Stock Sorting" table counts these by the hour).
CREATE TABLE stock_sorts (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `at` DATETIME NOT NULL,
  `operator` VARCHAR(100) NOT NULL DEFAULT '',
  `source` VARCHAR(40) NOT NULL DEFAULT '',
  `ref` VARCHAR(100) NOT NULL DEFAULT '',
  `stock_no` VARCHAR(100) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `ix_stock_sorts_at` (`at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
