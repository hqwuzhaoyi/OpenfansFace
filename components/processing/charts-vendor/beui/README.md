# BEUI chart sources for the processing dashboard

Official MIT sources: https://beui.dev/charts/bar-chart and https://beui.dev/charts/status-bar.
The upstream license is retained verbatim in LICENSE. Exact registry payloads and per-file upstream hashes are in `.local/experiences/chart-component-audit-20261108/` and `.local/experiences/operations-charts-20261108/vendor-manifest.json`.

Only the transitive import closure for BarChart and StatusBar is included (not their example pages). Registry @/ imports are converted to relative paths so this exploration does not overwrite shared production components. Chart contexts use reduced motion for this frequently updating dashboard; plot fades are immediate in that mode and status periods do not wait for an intersection observer to become visible. Plotting, keyboard inspection, tooltips, accessible data tables, legends and touch handling remain the official implementations.

The application declares @floating-ui/dom directly. DonutChart remains the existing shared implementation. Imports are relative within this support tree so existing shared buttons, tooltips and utilities are not overwritten.

Status plot rendering is immediate in reduced-motion mode, including its initial visible state. The root LICENSE and the per-file provenance manifest in .local/experiences/operations-charts-20261108 are retained.
