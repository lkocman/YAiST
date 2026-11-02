#!/bin/bash
# Builds the openSUSE Handbook (docs/handbook) as PDF, EPUB and HTML into
# target/handbook, in a throwaway Tumbleweed container. Tumbleweed packages
# asciidoctor but not its PDF and EPUB backends, so those come from
# rubygems. Arguments pick formats (default: pdf epub html).
# Update the generated parts first with `yaist docs`.
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# -gt 0 ] || set -- pdf epub html
mkdir -p target/handbook
exec podman run --rm --userns=keep-id --user root \
    -v "$PWD":/src:Z -w /src/docs/handbook \
    -e FORMATS="$*" -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    registry.opensuse.org/opensuse/tumbleweed:latest sh -euc '
        zypper -nq --gpg-auto-import-keys install --no-recommends ruby ruby-devel "rubygem(asciidoctor)" "rubygem(rouge)" gcc make inter-fonts python3-fonttools >/dev/null
        gem install --no-document --silent --bindir /usr/local/bin --no-format-executable asciidoctor-pdf asciidoctor-epub3 >/dev/null
        out=/src/target/handbook
        python3 /src/scripts/handbook-fonts.py /tmp/fonts
        for f in $FORMATS; do
            case $f in
                pdf)  asciidoctor-pdf --failure-level WARN -a pdf-fontsdir="/tmp/fonts;GEM_FONTS_DIR" -o "$out/opensuse-handbook.pdf" opensuse-handbook.adoc ;;
                epub) asciidoctor-epub3 --failure-level WARN -a ebook-validate! -o "$out/opensuse-handbook.epub" opensuse-handbook.adoc ;;
                html) asciidoctor --failure-level WARN -a imagesdir=screenshots -o "$out/index.html" opensuse-handbook.adoc && cp -r ../screenshots "$out/" ;;
                *) echo "unknown format $f" >&2; exit 2 ;;
            esac
        done
        chown -R "$HOST_UID:$HOST_GID" "$out"
    '
