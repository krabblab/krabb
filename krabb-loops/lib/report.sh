#!/usr/bin/env bash

validate_review_output() {
    python3 "$LOOP_DIR/lib/report.py" "$LOOP" "$1" "$2" || return 72
    read -r REPORT_COUNT REPORT_BLOCKERS REPORT_LIMITATIONS <"$2.counts"
}

has_pending_work() {
    (( REPORT_BLOCKERS > 0 || REPORT_LIMITATIONS > 0 ))
}
