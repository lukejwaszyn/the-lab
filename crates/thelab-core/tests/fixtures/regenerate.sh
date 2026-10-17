#!/bin/sh
# Regenerates the real TeX logs under logs/ from the projects under projects/
# (plus crates/examples/thesis). Needs tectonic, pdflatex and bibtex on PATH.
#
# Run it from anywhere:  sh crates/thelab-core/tests/fixtures/regenerate.sh
# Review the diff afterwards: engine upgrades change log text, and the parser
# tests pin specific lines.
set -u

here=$(cd "$(dirname "$0")" && pwd)
# The crates folder, which holds examples/thesis.
repo=$(cd "$here/../../.." && pwd)
logs="$here/logs"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$logs/tectonic" "$logs/pdflatex" "$logs/pdflatex-file-line-error"

# pdflatex names generated bitmap fonts by their path under TEXMFVAR, which
# defaults to the home directory. Point it somewhere neutral so that logs do
# not carry a user name.
# These /tmp paths keep the project's first name, Change: the recorded logs
# and SyncTeX data hold them, so renaming them means regenerating everything.
export TEXMFVAR=/tmp/change-fixtures-texmf-var

# Pin the date pdflatex prints in the log banner (2026-10-11 00:00 UTC), so
# that regenerating without other changes produces no diff.
export SOURCE_DATE_EPOCH=1791676800
export FORCE_SOURCE_DATE=1

run_project() {
    name=$1
    src=$2

    # Tectonic: the kept log, and stderr, which carries its errors and
    # warnings. stdout is only progress notes in an order that varies from
    # run to run, so it is not kept.
    rm -rf "$work/t" && cp -R "$src" "$work/t" && rm -rf "$work/t/build" && mkdir -p "$work/t/build"
    (cd "$work/t" && tectonic -X compile main.tex --synctex --keep-logs --outdir build \
        >/dev/null 2>"$logs/tectonic/$name.stderr.txt")
    echo "tectonic  $name: exit $?"
    for ext in log blg; do
        if [ -f "$work/t/build/main.$ext" ]; then
            cp "$work/t/build/main.$ext" "$logs/tectonic/$name.$ext"
        else
            rm -f "$logs/tectonic/$name.$ext"
        fi
    done
    [ -s "$logs/tectonic/$name.stderr.txt" ] || rm -f "$logs/tectonic/$name.stderr.txt"

    # pdflatex, with and without -file-line-error. Two passes plus bibtex so
    # that reference warnings are the ones that survive a real build.
    for variant in pdflatex pdflatex-file-line-error; do
        flags="-interaction=nonstopmode"
        [ "$variant" = pdflatex-file-line-error ] && flags="$flags -file-line-error"
        rm -rf "$work/p" && cp -R "$src" "$work/p" && rm -rf "$work/p/build"
        (
            cd "$work/p" || exit 1
            pdflatex $flags main.tex >/dev/null 2>&1
            if grep -q '\\bibdata' main.aux 2>/dev/null; then bibtex main >/dev/null 2>&1; fi
            pdflatex $flags main.tex >/dev/null 2>&1
            pdflatex $flags main.tex >/dev/null 2>&1
        )
        echo "$variant  $name: done"
        cp "$work/p/main.log" "$logs/$variant/$name.log"
        if [ "$variant" = pdflatex ] && [ -f "$work/p/main.blg" ]; then
            cp "$work/p/main.blg" "$logs/pdflatex/$name.blg"
        fi
    done
}

for dir in "$here"/projects/*/; do
    run_project "$(basename "$dir")" "$dir"
done
run_project thesis "$repo/examples/thesis"

# SyncTeX data for the example thesis, from both engines. SyncTeX records
# absolute paths, so this runs in a fixed folder: the tests name it.
sync=/tmp/change-fixtures-thesis
mkdir -p "$here/synctex"
for engine in tectonic pdflatex; do
    rm -rf "$sync" && cp -R "$repo/examples/thesis" "$sync" && rm -rf "$sync/build"
    (
        cd "$sync" || exit 1
        if [ "$engine" = tectonic ]; then
            mkdir build
            tectonic -X compile main.tex --synctex --outdir build >/dev/null 2>&1
            gzip -dc build/main.synctex.gz
        else
            pdflatex -interaction=nonstopmode -synctex=1 main.tex >/dev/null 2>&1
            bibtex main >/dev/null 2>&1
            pdflatex -interaction=nonstopmode -synctex=1 main.tex >/dev/null 2>&1
            pdflatex -interaction=nonstopmode -synctex=1 main.tex >/dev/null 2>&1
            gzip -dc main.synctex.gz
        fi
    ) >"$here/synctex/thesis.$engine.synctex"
    echo "synctex  $engine: $(wc -l <"$here/synctex/thesis.$engine.synctex") lines"
done
rm -rf "$sync"
