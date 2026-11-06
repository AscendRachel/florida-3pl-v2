-- Everything that belongs to the orders besides the orders table itself.
-- (In the old app these were separate SQLite tables and JSON files, keyed by "rn" = the row number and "col" = a column number.
--  Here a column is named by its field key, e.g. `shipped_to_customer`, and the tables point at orders with real foreign keys.)

-- What the All Orders sheet shows for each column: its label, kind and place. The 54 built-in fields are rows here too, so an admin can
-- rename or hide them; a column added by an admin has custom = 1 and keeps its values in order_custom_values.
CREATE TABLE column_defs (
  `id` INT UNSIGNED NOT NULL AUTO_INCREMENT,
  `field` VARCHAR(64) NOT NULL COMMENT 'the key: the orders column name, or c_<id> for a custom column',
  `label` VARCHAR(255) NOT NULL,
  `kind` ENUM('text','num','date','bool','select') NOT NULL DEFAULT 'text',
  `position` INT NOT NULL DEFAULT 0,
  `custom` TINYINT(1) NOT NULL DEFAULT 0,
  `deleted` TINYINT(1) NOT NULL DEFAULT 0,
  `deleted_by` VARCHAR(64) NULL,
  `deleted_at` DATETIME NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uq_field` (`field`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE order_custom_values (
  `rn` BIGINT UNSIGNED NOT NULL,
  `column_id` INT UNSIGNED NOT NULL,
  `value` TEXT NULL,
  PRIMARY KEY (`rn`, `column_id`),
  CONSTRAINT `fk_ocv_order` FOREIGN KEY (`rn`) REFERENCES orders (`rn`) ON DELETE CASCADE,
  CONSTRAINT `fk_ocv_column` FOREIGN KEY (`column_id`) REFERENCES column_defs (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dropdown lists (the sheet's data validation) and which cells use which list.
CREATE TABLE dropdown_lists (
  `id` INT UNSIGNED NOT NULL AUTO_INCREMENT,
  `options` JSON NOT NULL,
  `strict` TINYINT(1) NOT NULL DEFAULT 0,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE dropdown_cells (
  `rn` BIGINT UNSIGNED NOT NULL,
  `field` VARCHAR(64) NOT NULL,
  `list_id` INT UNSIGNED NOT NULL,
  PRIMARY KEY (`rn`, `field`),
  KEY `ix_dc_list` (`list_id`),
  CONSTRAINT `fk_dc_order` FOREIGN KEY (`rn`) REFERENCES orders (`rn`) ON DELETE CASCADE,
  CONSTRAINT `fk_dc_list` FOREIGN KEY (`list_id`) REFERENCES dropdown_lists (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Cells that were edited by hand in the app (kept so a later import does not lose them).
CREATE TABLE order_edits (
  `rn` BIGINT UNSIGNED NOT NULL,
  `field` VARCHAR(64) NOT NULL,
  `value` TEXT NULL,
  `at` DATETIME NOT NULL,
  PRIMARY KEY (`rn`, `field`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- The history of every change to a cell (who, when, old value, new value, why).
CREATE TABLE edit_log (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `at` DATETIME NOT NULL,
  `who` VARCHAR(100) NOT NULL DEFAULT '',
  `rn` BIGINT UNSIGNED NOT NULL,
  `field` VARCHAR(64) NOT NULL,
  `old_value` TEXT NULL,
  `new_value` TEXT NULL,
  `note` VARCHAR(255) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `ix_edit_log_cell` (`rn`, `field`),
  KEY `ix_edit_log_at` (`at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Every package scan in the Scan tab, and its outcome.
CREATE TABLE scans (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `at` DATETIME NOT NULL,
  `operator` VARCHAR(100) NOT NULL DEFAULT '',
  `code` VARCHAR(255) NOT NULL DEFAULT '',
  `rn` BIGINT UNSIGNED NULL,
  `outcome` VARCHAR(40) NOT NULL DEFAULT '',
  `note` TEXT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_scans_at` (`at`),
  KEY `ix_scans_rn` (`rn`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Who shipped what and when (the Dashboard's hourly tables count these).
CREATE TABLE ship_events (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `rn` BIGINT UNSIGNED NOT NULL,
  `kind` ENUM('customer','amazon') NOT NULL,
  `at` DATETIME NOT NULL,
  `operator` VARCHAR(100) NOT NULL DEFAULT '',
  `note` VARCHAR(255) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `ix_ship_events` (`rn`, `kind`, `at`),
  KEY `ix_ship_events_at` (`kind`, `at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- The boxes counted for an order (a package that arrives in several boxes).
CREATE TABLE boxes (
  `rn` BIGINT UNSIGNED NOT NULL,
  `code` VARCHAR(255) NOT NULL,
  `at` DATETIME NOT NULL,
  PRIMARY KEY (`rn`, `code`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Deleted orders, kept so they can be restored (the whole row as JSON).
CREATE TABLE trash_rows (
  `rn` BIGINT UNSIGNED NOT NULL,
  `label` VARCHAR(255) NULL,
  `product` TEXT NULL,
  `customer` VARCHAR(255) NULL,
  `data` JSON NOT NULL,
  `deleted_by` VARCHAR(100) NOT NULL DEFAULT '',
  `deleted_at` DATETIME NOT NULL,
  PRIMARY KEY (`rn`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Small settings of the app, one row each: branding, the Dashboard's operators, column and row locks, the last import.
CREATE TABLE app_settings (
  `k` VARCHAR(64) NOT NULL,
  `v` JSON NOT NULL,
  PRIMARY KEY (`k`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
