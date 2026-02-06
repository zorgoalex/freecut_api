#!/usr/bin/env python3
import unittest
from unittest.mock import patch

import greedy_optimize as g


def make_item(item_id: str, w: float = 1000.0, h: float = 500.0) -> g.Item:
    return g.Item(
        id=item_id,
        width_mm=w,
        height_mm=h,
        qty=1,
        rotation="allow_90",
        pattern_direction="none",
    )


class GreedyOptimizeTests(unittest.TestCase):
    class FakeResponse:
        def __init__(self, status_code: int, payload: dict | None = None) -> None:
            self.status_code = status_code
            self._payload = payload or {}

        def json(self) -> dict:
            return self._payload

    def test_extract_placed_item_ids_partial_subset(self) -> None:
        subset = [make_item("L#1"), make_item("L#2"), make_item("L#3")]
        placements = [
            {"item_id": "L", "instance": 1},
            {"item_id": "L", "instance": 2},
        ]

        placed_ids = g.extract_placed_item_ids(subset, placements)

        self.assertEqual(len(placed_ids), 2)
        self.assertEqual(set(placed_ids), {"L#1", "L#2"})

    def test_optimize_single_sheet_skips_empty_solutions(self) -> None:
        remaining = [make_item("overs#1", 3000.0, 500.0)]
        stock = {"id": "mdf_2800x2070", "width_mm": 2800.0, "height_mm": 2070.0}
        params = {
            "kerf_mm": 2.0,
            "spacing_mm": 1.0,
            "trim_mm": {"left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0},
            "objective": "min_waste",
            "layout_mode": "guillotine",
        }

        with patch.object(
            g,
            "optimize_subset",
            return_value=(
                {
                    "status": "ok",
                    "summary": {"waste_percent": 0.0},
                    "solutions": [],
                    "unplaced_items": [{"item_id": "overs", "instance": 1, "reason": "oversized"}],
                },
                "200",
            ),
        ):
            result = g.optimize_single_sheet(
                remaining_items=remaining,
                stock=stock,
                params=params,
                runtime=g.RuntimeOptions(),
                sheet_area=g.calculate_sheet_area(stock, params["trim_mm"]),
                strategy="largest_first",
                max_iterations=1,
                early_stop=1,
                verbose=False,
            )

        self.assertIsNone(result)

    def test_greedy_removes_only_placed_items(self) -> None:
        items = [
            {"id": "L", "width_mm": 1700.0, "height_mm": 1100.0, "qty": 3, "rotation": "allow_90", "pattern_direction": "none"},
        ]
        stock = {"id": "mdf_2800x2070", "width_mm": 2800.0, "height_mm": 2070.0, "qty": 1}
        params = {
            "kerf_mm": 2.0,
            "spacing_mm": 1.0,
            "trim_mm": {"left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0},
            "objective": "min_waste",
            "layout_mode": "guillotine",
        }

        side_effect = [
            g.SheetResult(
                stock_id="mdf_2800x2070",
                placements=[
                    {"item_id": "L", "instance": 1, "width_mm": 1700.0, "height_mm": 1100.0},
                    {"item_id": "L", "instance": 2, "width_mm": 1700.0, "height_mm": 1100.0},
                ],
                waste_percent=35.0,
                placed_item_ids=["L#1", "L#2"],
                selected_count=3,
                seed_used=123,
            ),
            None,
        ]

        with patch.object(g, "optimize_single_sheet", side_effect=side_effect):
            result = g.greedy_optimize(
                items_config=items,
                stock_input=stock,
                params=params,
                strategy="largest_first",
                total_time_limit=2,
                verbose=False,
            )

        summary = result["summary"]
        self.assertEqual(summary["items_total"], 3)
        self.assertEqual(summary["items_placed"], 2)
        self.assertEqual(len(result["unplaced_items"]), 1)
        self.assertTrue(summary["invariant_ok"])
        self.assertEqual(summary["missing_items"], 0)
        self.assertEqual(result["unplaced_items"][0]["id"], "L#3")

    def test_optimize_subset_retries_429_once(self) -> None:
        item = make_item("A#1", 800.0, 600.0)
        stock = {"id": "mdf_2800x2070", "width_mm": 2800.0, "height_mm": 2070.0}
        params = {
            "kerf_mm": 2.0,
            "spacing_mm": 1.0,
            "trim_mm": {"left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0},
            "objective": "min_waste",
            "layout_mode": "guillotine",
        }
        runtime = g.RuntimeOptions(retry_429=1, retry_backoff_s=0.0)
        ok_payload = {
            "status": "ok",
            "summary": {"waste_percent": 1.23},
            "solutions": [{"placements": [{"item_id": "A", "instance": 1}]}],
        }

        with patch.object(
            g.requests,
            "post",
            side_effect=[self.FakeResponse(429), self.FakeResponse(200, ok_payload)],
        ) as mocked_post:
            result, status = g.optimize_subset([item], stock, params, seed=123, runtime=runtime)

        self.assertEqual(status, "200")
        self.assertIsNotNone(result)
        self.assertEqual(mocked_post.call_count, 2)

    def test_optimize_single_sheet_stops_on_consecutive_408(self) -> None:
        remaining = [make_item("A#1"), make_item("A#2")]
        stock = {"id": "mdf_2800x2070", "width_mm": 2800.0, "height_mm": 2070.0}
        params = {
            "kerf_mm": 2.0,
            "spacing_mm": 1.0,
            "trim_mm": {"left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0},
            "objective": "min_waste",
            "layout_mode": "guillotine",
        }
        runtime = g.RuntimeOptions(max_consecutive_408=2)
        api_stats = {}

        with patch.object(g, "optimize_subset", return_value=(None, "408")) as mocked:
            result = g.optimize_single_sheet(
                remaining_items=remaining,
                stock=stock,
                params=params,
                runtime=runtime,
                sheet_area=g.calculate_sheet_area(stock, params["trim_mm"]),
                strategy="largest_first",
                max_iterations=10,
                early_stop=10,
                api_status_counts=api_stats,
                verbose=False,
            )

        self.assertIsNone(result)
        self.assertEqual(mocked.call_count, 2)
        self.assertEqual(api_stats.get("408"), 2)

    def test_item_fits_sheet_rotation_and_gap(self) -> None:
        stock = {"id": "mdf_2800x2070", "width_mm": 2800.0, "height_mm": 2070.0}
        params = {
            "kerf_mm": 2.0,
            "spacing_mm": 1.0,
            "trim_mm": {"left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0},
        }
        fits_rotated = make_item("R#1", 2047.0, 2700.0)
        fits_rotated.rotation = "allow_90"
        no_fit = make_item("X#1", 3000.0, 500.0)

        self.assertTrue(g.item_fits_sheet(fits_rotated, stock, params))
        self.assertFalse(g.item_fits_sheet(no_fit, stock, params))

    def test_greedy_prefit_filters_oversized(self) -> None:
        items = [
            {"id": "overs", "width_mm": 3000.0, "height_mm": 500.0, "qty": 1, "rotation": "allow_90", "pattern_direction": "none"},
        ]
        stock = {"id": "mdf_2800x2070", "width_mm": 2800.0, "height_mm": 2070.0, "qty": 1}
        params = {
            "kerf_mm": 2.0,
            "spacing_mm": 1.0,
            "trim_mm": {"left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0},
            "objective": "min_waste",
            "layout_mode": "guillotine",
        }

        with patch.object(g, "optimize_single_sheet") as mocked:
            result = g.greedy_optimize(
                items_config=items,
                stock_input=stock,
                params=params,
                strategy="largest_first",
                total_time_limit=1,
                verbose=False,
            )

        self.assertEqual(mocked.call_count, 0)
        self.assertEqual(result["summary"]["prefit_filtered_items"], 1)
        self.assertEqual(len(result["unplaced_items"]), 1)

    def test_greedy_respects_stock_qty_limit(self) -> None:
        items = [
            {"id": "A", "width_mm": 1000.0, "height_mm": 800.0, "qty": 3, "rotation": "allow_90", "pattern_direction": "none"},
        ]
        stock = {"id": "mdf_2800x2070", "width_mm": 2800.0, "height_mm": 2070.0, "qty": 1}
        params = {
            "kerf_mm": 2.0,
            "spacing_mm": 1.0,
            "trim_mm": {"left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0},
            "objective": "min_waste",
            "layout_mode": "guillotine",
        }

        with patch.object(
            g,
            "optimize_single_sheet",
            return_value=g.SheetResult(
                stock_id="mdf_2800x2070",
                placements=[{"item_id": "A", "instance": 1, "width_mm": 1000.0, "height_mm": 800.0}],
                waste_percent=80.0,
                placed_item_ids=["A#1"],
                selected_count=3,
                seed_used=1,
                stop_reason="no_improve",
            ),
        ):
            result = g.greedy_optimize(
                items_config=items,
                stock_input=stock,
                params=params,
                strategy="largest_first",
                total_time_limit=2,
                verbose=False,
            )

        self.assertEqual(result["summary"]["sheets_used"], 1)
        self.assertEqual(result["summary"]["stock_usage"]["mdf_2800x2070"], 1)
        self.assertEqual(len(result["unplaced_items"]), 2)

    def test_greedy_uses_alternative_stock_after_qty_exhausted(self) -> None:
        items = [
            {"id": "A", "width_mm": 1000.0, "height_mm": 800.0, "qty": 2, "rotation": "allow_90", "pattern_direction": "none"},
        ]
        stocks = [
            {"id": "small", "width_mm": 2000.0, "height_mm": 1200.0, "qty": 1},
            {"id": "mdf_2800x2070", "width_mm": 2800.0, "height_mm": 2070.0, "qty": 0},
        ]
        params = {
            "kerf_mm": 2.0,
            "spacing_mm": 1.0,
            "trim_mm": {"left": 10.0, "right": 10.0, "top": 10.0, "bottom": 10.0},
            "objective": "min_waste",
            "layout_mode": "guillotine",
        }

        def fake_optimize_single_sheet(remaining_items, stock, *_args, **_kwargs):
            if not remaining_items:
                return None
            item = remaining_items[0]
            return g.SheetResult(
                stock_id=stock["id"],
                placements=[{"item_id": "A", "instance": 1, "width_mm": item.width_mm, "height_mm": item.height_mm}],
                waste_percent=50.0,
                placed_item_ids=[item.id],
                selected_count=1,
                seed_used=7,
                stop_reason="no_improve",
            )

        with patch.object(g, "optimize_single_sheet", side_effect=fake_optimize_single_sheet):
            result = g.greedy_optimize(
                items_config=items,
                stock_input=stocks,
                params=params,
                strategy="largest_first",
                total_time_limit=2,
                verbose=False,
            )

        stock_usage = result["summary"]["stock_usage"]
        self.assertEqual(stock_usage["small"], 1)
        self.assertEqual(stock_usage["mdf_2800x2070"], 1)
        self.assertEqual(result["summary"]["items_placed"], 2)


if __name__ == "__main__":
    unittest.main()
