-- The standard columns of All Orders. A database made from scratch has none (only the old-data import fills column_defs), and the
-- pages need them; without them the Dashboard and All Orders fail. Does nothing when the table already has rows (an existing database).
-- The six "~deleted N~" counters are in the deleted state, as in the old app.
INSERT INTO column_defs (`field`, `header`, `label`, `kind`, `position`, `custom`, `deleted`)
SELECT t.f, t.h, t.l, t.k, t.p, 0, t.d FROM (
SELECT 'label_number' AS f, 'Label Number' AS h, 'Label Number' AS l, 'text' AS k, 0 AS p, 0 AS d
UNION ALL SELECT 'gift_note' AS f, 'Gift Note' AS h, 'Gift Note' AS l, 'text' AS k, 1 AS p, 0 AS d
UNION ALL SELECT 'store_order_no' AS f, 'Store Order No.' AS h, 'Store Order No.' AS l, 'text' AS k, 2 AS p, 0 AS d
UNION ALL SELECT 'product_name' AS f, 'Product Name' AS h, 'Product Name' AS l, 'text' AS k, 3 AS p, 0 AS d
UNION ALL SELECT 'brand' AS f, 'Brand' AS h, 'Brand' AS l, 'text' AS k, 4 AS p, 0 AS d
UNION ALL SELECT 'qty' AS f, 'QTY' AS h, 'QTY' AS l, 'num' AS k, 5 AS p, 0 AS d
UNION ALL SELECT 'customer_name' AS f, 'Customer Name' AS h, 'Customer Name' AS l, 'text' AS k, 6 AS p, 0 AS d
UNION ALL SELECT 'address' AS f, 'Address' AS h, 'Address' AS l, 'text' AS k, 7 AS p, 0 AS d
UNION ALL SELECT 'source_order_date' AS f, 'Source Order Date' AS h, 'Source Order Date' AS l, 'date' AS k, 8 AS p, 0 AS d
UNION ALL SELECT 'source_order_number' AS f, 'Source Order Number' AS h, 'Source Order Number' AS l, 'text' AS k, 9 AS p, 0 AS d
UNION ALL SELECT 'source_tracking_number' AS f, 'Source Tracking Number' AS h, 'Source Tracking Number' AS l, 'text' AS k, 10 AS p, 0 AS d
UNION ALL SELECT 'shipping_label_link' AS f, 'Shipping Label Link' AS h, 'Shipping Label Link' AS l, 'link' AS k, 11 AS p, 0 AS d
UNION ALL SELECT 'product_image_link' AS f, 'Product Image Link' AS h, 'Product Image Link' AS l, 'img' AS k, 12 AS p, 0 AS d
UNION ALL SELECT 'source_order_status' AS f, 'Source Order Status' AS h, 'Source Order Status' AS l, 'select' AS k, 13 AS p, 0 AS d
UNION ALL SELECT 'package_not_received' AS f, 'Package Not Received' AS h, 'Package Not Received' AS l, 'bool' AS k, 14 AS p, 0 AS d
UNION ALL SELECT 'to_void' AS f, 'To Void' AS h, 'To Void' AS l, 'bool' AS k, 15 AS p, 0 AS d
UNION ALL SELECT 'item_on_hold' AS f, 'ITEM ON HOLD' AS h, 'ITEM ON HOLD' AS l, 'bool' AS k, 16 AS p, 0 AS d
UNION ALL SELECT 'stock' AS f, 'Stock' AS h, 'Stock' AS l, 'bool' AS k, 17 AS p, 0 AS d
UNION ALL SELECT 'label_used' AS f, 'Label Used' AS h, 'Label Used' AS l, 'bool' AS k, 18 AS p, 0 AS d
UNION ALL SELECT 'shipped_to_customer' AS f, 'Shipped to Customer' AS h, 'Shipped to Customer' AS l, 'bool' AS k, 19 AS p, 0 AS d
UNION ALL SELECT 'shipped_to_amazon' AS f, 'Shipped to Amazon' AS h, 'Shipped to Amazon' AS l, 'bool' AS k, 20 AS p, 0 AS d
UNION ALL SELECT 'label_voided' AS f, 'Label Voided' AS h, 'Label Voided' AS l, 'bool' AS k, 21 AS p, 0 AS d
UNION ALL SELECT 'tracking_number_to_customer' AS f, 'Tracking Number To Customer' AS h, 'Tracking Number To Customer' AS l, 'text' AS k, 22 AS p, 0 AS d
UNION ALL SELECT 'delivery_date' AS f, 'Delivery Date' AS h, 'Delivery Date' AS l, 'date' AS k, 23 AS p, 0 AS d
UNION ALL SELECT 'tracking_status' AS f, 'Tracking Status' AS h, 'Tracking Status' AS l, 'select' AS k, 24 AS p, 0 AS d
UNION ALL SELECT 'notes' AS f, 'Notes' AS h, 'Notes' AS l, 'text' AS k, 25 AS p, 0 AS d
UNION ALL SELECT 'store' AS f, 'Store' AS h, 'Store' AS l, 'select' AS k, 26 AS p, 0 AS d
UNION ALL SELECT 'ph_team_internal_notes' AS f, 'PH Team Internal Notes' AS h, 'PH Team Internal Notes' AS l, 'text' AS k, 27 AS p, 0 AS d
UNION ALL SELECT 'days_in_transit' AS f, 'Days In Transit' AS h, 'Days In Transit' AS l, 'text' AS k, 28 AS p, 0 AS d
UNION ALL SELECT 'number_of_orders' AS f, 'Number of Orders' AS h, 'Number of Orders' AS l, 'num' AS k, 29 AS p, 0 AS d
UNION ALL SELECT 'print_link' AS f, 'Print Link' AS h, 'Print Link' AS l, 'link' AS k, 30 AS p, 0 AS d
UNION ALL SELECT 'softr_record_id' AS f, '? Softr Record ID' AS h, '? Softr Record ID' AS l, 'text' AS k, 31 AS p, 0 AS d
UNION ALL SELECT 'number_of_items' AS f, 'Number of Items' AS h, 'Number of Items' AS l, 'num' AS k, 32 AS p, 0 AS d
UNION ALL SELECT 'boxes_received' AS f, 'Boxes Received' AS h, 'Boxes Received' AS l, 'num' AS k, 33 AS p, 0 AS d
UNION ALL SELECT 'boxes_expected' AS f, 'Boxes Expected' AS h, 'Boxes Expected' AS l, 'num' AS k, 34 AS p, 0 AS d
UNION ALL SELECT 'hold_warning' AS f, 'Hold Warning' AS h, 'Hold Warning' AS l, 'text' AS k, 35 AS p, 0 AS d
UNION ALL SELECT 'hold_date' AS f, 'Hold Date' AS h, 'Hold Date' AS l, 'date' AS k, 36 AS p, 0 AS d
UNION ALL SELECT 'sku' AS f, 'SKU' AS h, 'SKU' AS l, 'text' AS k, 37 AS p, 0 AS d
UNION ALL SELECT 'stock_no' AS f, 'STOCK NO.' AS h, 'STOCK NO.' AS l, 'text' AS k, 38 AS p, 0 AS d
UNION ALL SELECT 'tapicker_status' AS f, 'TAPICKER STATUS' AS h, 'TAPICKER STATUS' AS l, 'text' AS k, 39 AS p, 0 AS d
UNION ALL SELECT 'tapicker_shipped_date' AS f, 'TAPICKER SHIPPED DATE' AS h, 'TAPICKER SHIPPED DATE' AS l, 'date' AS k, 40 AS p, 0 AS d
UNION ALL SELECT 'tapicker_delivery_date' AS f, 'TAPICKER DELIVERY DATE' AS h, 'TAPICKER DELIVERY DATE' AS l, 'date' AS k, 41 AS p, 0 AS d
UNION ALL SELECT 'label_chip' AS f, 'Label Chip' AS h, 'Label Chip' AS l, 'text' AS k, 42 AS p, 0 AS d
UNION ALL SELECT 'shipped_to_customer_date' AS f, 'SHIPPED TO  CUSTOMER DATE' AS h, 'SHIPPED TO  CUSTOMER DATE' AS l, 'date' AS k, 43 AS p, 0 AS d
UNION ALL SELECT 'shipped_to_amazon_date' AS f, 'SHIPPED TO  AMAZON DATE' AS h, 'SHIPPED TO  AMAZON DATE' AS l, 'date' AS k, 44 AS p, 0 AS d
UNION ALL SELECT 'rts_counter' AS f, '~deleted 45~ RTS COUNTER' AS h, '~deleted 45~ RTS COUNTER' AS l, 'num' AS k, 45 AS p, 1 AS d
UNION ALL SELECT 'rta_counter' AS f, '~deleted 46~ RTA COUNTER' AS h, '~deleted 46~ RTA COUNTER' AS l, 'num' AS k, 46 AS p, 1 AS d
UNION ALL SELECT 'item_on_hold_counter' AS f, '~deleted 47~ Item on Hold Counter' AS h, '~deleted 47~ Item on Hold Counter' AS l, 'num' AS k, 47 AS p, 1 AS d
UNION ALL SELECT 'store_returns_counter' AS f, '~deleted 48~ Store Returns Counter' AS h, '~deleted 48~ Store Returns Counter' AS l, 'num' AS k, 48 AS p, 1 AS d
UNION ALL SELECT 'all_orders_counter' AS f, '~deleted 49~ All Orders Counter' AS h, '~deleted 49~ All Orders Counter' AS l, 'num' AS k, 49 AS p, 1 AS d
UNION ALL SELECT 'stock_counter' AS f, '~deleted 50~ Stock Counter' AS h, '~deleted 50~ Stock Counter' AS l, 'num' AS k, 50 AS p, 1 AS d
UNION ALL SELECT 'updated_by' AS f, 'Updated by' AS h, 'Updated by' AS l, 'text' AS k, 51 AS p, 0 AS d
UNION ALL SELECT 'delivered_to_warehouse_date' AS f, 'Delivered to Warehouse Date' AS h, 'Delivered to Warehouse Date' AS l, 'date' AS k, 52 AS p, 0 AS d
UNION ALL SELECT 'items_received' AS f, 'Items Received' AS h, 'Items Received' AS l, 'num' AS k, 53 AS p, 0 AS d
) t
WHERE NOT EXISTS (SELECT 1 FROM column_defs);