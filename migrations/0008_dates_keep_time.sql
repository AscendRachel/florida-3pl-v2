-- The sheet may hold a time in these two date columns (typed in, or from an Excel date-time). Keep it: a time of exactly 00:00 shows as a date only.
ALTER TABLE orders MODIFY `source_order_date` DATETIME NULL;
ALTER TABLE orders MODIFY `delivery_date` DATETIME NULL;
