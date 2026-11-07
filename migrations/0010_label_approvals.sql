-- A person checked a flagged Shipping Label Link on the Admin tab and approved it. It is tied to the order row AND the file (the Drive file id),
-- so if the link is changed to another file, the new file is checked again. A "replace everything" import clears these (row numbers start over).
CREATE TABLE label_approvals (
  `rn` BIGINT UNSIGNED NOT NULL,
  `file_id` VARCHAR(100) NOT NULL,
  `approved_by` VARCHAR(64) NOT NULL,
  `approved_at` DATETIME NOT NULL,
  PRIMARY KEY (`rn`, `file_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
