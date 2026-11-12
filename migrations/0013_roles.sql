-- Roles the admins can add and remove (Admin, Warehouse and Member stay). A role may carry a default set of tabs that is offered when it is given to someone.
ALTER TABLE users MODIFY `role` VARCHAR(20) NOT NULL DEFAULT 'member';

CREATE TABLE roles (
  `name` VARCHAR(20) NOT NULL COMMENT 'the short key kept on each person (letters and numbers)',
  `label` VARCHAR(40) NOT NULL COMMENT 'what people see',
  `tabs` JSON NULL COMMENT 'the default tabs offered with this role; NULL = none chosen',
  `builtin` TINYINT(1) NOT NULL DEFAULT 0,
  `pos` INT NOT NULL DEFAULT 0,
  PRIMARY KEY (`name`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

INSERT INTO roles (`name`, `label`, `builtin`, `pos`) VALUES ('admin', 'Admin', 1, 1), ('warehouse', 'Warehouse', 1, 2), ('member', 'Member', 1, 3);
