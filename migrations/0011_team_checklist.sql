-- The Team Daily Check List: who does which daily task (one colour per agent), what was ticked each day, and the two free lists beside it.

CREATE TABLE checklist_agents (
  `agent` VARCHAR(20) NOT NULL,
  `name` VARCHAR(60) NOT NULL,
  `color` VARCHAR(9) NOT NULL COMMENT 'the colour of the agent (the legend)',
  `pos` INT NOT NULL DEFAULT 0,
  PRIMARY KEY (`agent`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE checklist_tasks (
  `code` CHAR(1) NOT NULL COMMENT 'A, B, C ...',
  `title` VARCHAR(400) NOT NULL,
  `agent` VARCHAR(20) NOT NULL COMMENT 'who is assigned: the row takes this agent''s colour',
  `pos` INT NOT NULL DEFAULT 0,
  PRIMARY KEY (`code`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- one row per task and day: the three ticks (and who made each), and the typed number of the tasks that are not counted by the app
CREATE TABLE checklist_marks (
  `day` DATE NOT NULL,
  `task` CHAR(1) NOT NULL,
  `doing` TINYINT(1) NOT NULL DEFAULT 0,
  `doing_by` VARCHAR(100) NOT NULL DEFAULT '',
  `done` TINYINT(1) NOT NULL DEFAULT 0,
  `done_by` VARCHAR(100) NOT NULL DEFAULT '',
  `remarks` TINYINT(1) NOT NULL DEFAULT 0,
  `remarks_by` VARCHAR(100) NOT NULL DEFAULT '',
  `number_text` VARCHAR(2000) NOT NULL DEFAULT '',
  PRIMARY KEY (`day`, `task`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- the counted numbers that are "right now" (not by day), kept as they last were on each day
CREATE TABLE checklist_counts (
  `day` DATE NOT NULL,
  `task` CHAR(1) NOT NULL,
  `n` BIGINT NOT NULL,
  PRIMARY KEY (`day`, `task`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- the "Additional Task" and "SKU with more than one box" lists (they stay until somebody deletes a line)
CREATE TABLE checklist_items (
  `id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `kind` ENUM('additional','sku') NOT NULL,
  `text` VARCHAR(2000) NOT NULL DEFAULT '',
  `updated_by` VARCHAR(100) NOT NULL DEFAULT '',
  `updated_at` DATETIME NOT NULL,
  PRIMARY KEY (`id`),
  KEY `ix_ci_kind` (`kind`, `id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

INSERT INTO checklist_agents (agent, name, color, pos) VALUES
  ('flordeliza', 'FLORDELIZA', '#00ff00', 1),
  ('lhoyd', 'LHOYD', '#ffff00', 2),
  ('johnriel', 'JOHNRIEL', '#00ffff', 3),
  ('maricon', 'MARICON', '#fff2cc', 4);

INSERT INTO checklist_tasks (code, title, agent, pos) VALUES
  ('A', 'Check Tracking on Amazon (Orders with No Tracking Yet) – Julian Small Item and Shipped to Warehouse and', 'flordeliza', 1),
  ('B', 'Update Source Order Status', 'flordeliza', 2),
  ('C', 'Update Tracking Status (USPS, UPS, FedEx) (prerequisite to D) (Green Rows)', 'lhoyd', 3),
  ('D', 'Identify Rows for Orange Issue (Assumed Not Received) (prerequisite to E)', 'johnriel', 4),
  ('E', 'Actions for Orange Rows', 'lhoyd', 5),
  ('F', 'Actions for Red Rows', 'flordeliza', 6),
  ('G', 'Actions for Magenta Rows', 'johnriel', 7),
  ('H', 'Actions for Refund', 'johnriel', 8),
  ('I', 'Check Cancelled Orders from Source', 'johnriel', 9),
  ('J', 'Check Payment Revisions', 'lhoyd', 10),
  ('K', 'Report Order Issue Notifications to SME', 'lhoyd', 11),
  ('L', 'Actioning SME Concern Tickets', 'johnriel', 12),
  ('M', 'Process Returns', 'johnriel', 13),
  ('N', 'Clearing Customer Portal Cards in Mathew''s Software', 'lhoyd', 14),
  ('O', 'Update brand and image link.', 'flordeliza', 15),
  ('P', 'Check the grey or in-transit rows that appears to be stagnant.', 'flordeliza', 16),
  ('Q', 'Monitor Replacements', 'johnriel', 17);
