-- People, sessions, the Work Log chat, notifications and the Google Drive file names.

CREATE TABLE users (
  `username` VARCHAR(64) NOT NULL,
  `name` VARCHAR(100) NOT NULL,
  `pw_hash` VARCHAR(255) NOT NULL COMMENT 'PBKDF2-SHA256 hex, 200000 rounds with the salt below (the same as the Python app, so passwords carry over)',
  `salt` VARCHAR(64) NOT NULL DEFAULT '' COMMENT 'random hex salt of this person',
  `role` ENUM('admin','warehouse','member') NOT NULL DEFAULT 'member',
  `tabs` JSON NULL COMMENT 'the tabs this person may open; NULL = all',
  `disabled` TINYINT(1) NOT NULL DEFAULT 0,
  `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`username`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE sessions (
  `token` VARCHAR(128) NOT NULL,
  `username` VARCHAR(64) NOT NULL,
  `expires_at` DATETIME NOT NULL,
  PRIMARY KEY (`token`),
  KEY `ix_sessions_user` (`username`),
  KEY `ix_sessions_expires` (`expires_at`),
  CONSTRAINT `fk_sessions_user` FOREIGN KEY (`username`) REFERENCES users (`username`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- The floating chat bubble: what each person wrote, with the date and time (admins read it in the Work Log).
CREATE TABLE work_log_messages (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `at` DATETIME NOT NULL,
  `username` VARCHAR(64) NOT NULL,
  `name` VARCHAR(100) NOT NULL DEFAULT '',
  `text` VARCHAR(1000) NOT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_wl_at` (`at`),
  KEY `ix_wl_user` (`username`, `at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- The bell: a note somebody wrote on an order. "kind" says whether it answered or rewrote another person's note.
CREATE TABLE notifications (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `at` DATETIME NOT NULL,
  `by_user` VARCHAR(64) NOT NULL,
  `by_name` VARCHAR(100) NOT NULL DEFAULT '',
  `by_role` VARCHAR(20) NOT NULL DEFAULT '',
  `rn` BIGINT UNSIGNED NOT NULL,
  `label` VARCHAR(255) NOT NULL DEFAULT '',
  `customer` VARCHAR(255) NOT NULL DEFAULT '',
  `field` VARCHAR(64) NOT NULL DEFAULT 'notes',
  `text` VARCHAR(600) NOT NULL DEFAULT '',
  `kind` ENUM('','reply','rewrite') NOT NULL DEFAULT '',
  `to_user` VARCHAR(64) NOT NULL DEFAULT '',
  `to_name` VARCHAR(100) NOT NULL DEFAULT '',
  PRIMARY KEY (`id`),
  KEY `ix_notif_rn` (`rn`, `field`, `id`),
  KEY `ix_notif_at` (`at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE notification_seen (
  `username` VARCHAR(64) NOT NULL,
  `last_id` BIGINT UNSIGNED NOT NULL DEFAULT 0,
  PRIMARY KEY (`username`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- The names of the Google Drive files behind the Shipping Label Link column (read in the background; used by the label mismatch check).
CREATE TABLE drive_names (
  `id` VARCHAR(128) NOT NULL,
  `name` VARCHAR(500) NOT NULL DEFAULT '',
  `read_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
