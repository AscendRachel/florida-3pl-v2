-- Source Time Frame: an order marked resolved leaves the table. up_to_id is the last Delivery Date change that was resolved,
-- so only a later change of the ETA brings the order back (counted from that point).
CREATE TABLE timeframe_resolved (
  `rn` BIGINT UNSIGNED NOT NULL,
  `up_to_id` BIGINT NOT NULL,
  `resolved_by` VARCHAR(100) NOT NULL DEFAULT '',
  `resolved_at` DATETIME NOT NULL,
  PRIMARY KEY (`rn`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
