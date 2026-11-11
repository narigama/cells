# cells

Terminal spreadsheet built in Rust with Excel-compatible formulas, mouse-driven menus, software pixel cursors, and live charts.

## Usage

```bash
cargo build --release

cells                    # New workbook
cells report.xlsx        # Open .xlsx, .csv, .tsv, .json, .jsonl, .parquet, .sqlite
cat sales.csv | cells    # Load piped CSV/TSV/JSON from stdin
```
