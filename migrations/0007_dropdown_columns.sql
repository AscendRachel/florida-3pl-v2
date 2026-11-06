-- A column an admin added can be a dropdown: every order gets the same list of choices (new rows too).
ALTER TABLE column_defs ADD COLUMN `dropdown` TINYINT(1) NOT NULL DEFAULT 0;
ALTER TABLE column_defs ADD COLUMN `options` JSON NULL;
