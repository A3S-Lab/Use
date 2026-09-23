# Build input for an analysis image. Desktop must not fetch these packages.
# The published OCI digest, not this file, is the installable identity.
# Packages stage is not published. It may inherit the base image toolchain.
# science-r-debs.tar is the snapshot closure staged beside this file.
# The guest does not apt-get or wget that closure: both fault on this filesystem.
# Compilers, git, and node are not installed. This is not conda.
# Version checks use Rscript: on a case-insensitive build rootfs, R is littler.
FROM r-base:4.5.3 AS packages
COPY science-r-debs.tar /tmp/science-r-debs.tar
RUN set -eu; \
    mkdir -p /tmp/a3s-debs /export/usr/local/bin /export/usr/local/share/a3s; \
    tar --warning=no-unknown-keyword -xf /tmp/science-r-debs.tar -C /tmp/a3s-debs; \
    test -n "$(find /tmp/a3s-debs -name 'r-cran-tidyverse_2.0.0*.deb' -print -quit)"; \
    fonts_deb=$(find /tmp/a3s-debs -name 'fonts-dejavu_2.37*.deb' -print -quit); \
    test -n "$fonts_deb"; \
    fonts_upstream=$(basename "$fonts_deb" | sed 's/^fonts-dejavu_//; s/[-+].*//'); \
    test "$fonts_upstream" = "2.37"; \
    find /tmp/a3s-debs -name '*.deb' -exec basename {} \; | sed 's/_.*//' | sort > /export/usr/local/share/a3s/new-packages.txt; \
    if grep -E '^(gcc|g\+\+|gfortran|cpp|clang|git|nodejs|node)$|^(gcc|g\+\+|gfortran|cpp|clang)-[0-9]+$' /export/usr/local/share/a3s/new-packages.txt; then echo 'refusing to install a toolchain into the analysis image' >&2; exit 1; fi; \
    for deb in /tmp/a3s-debs/*.deb; do dpkg-deb -x "$deb" /export; done; \
    LD_LIBRARY_PATH=/export/usr/lib/aarch64-linux-gnu:/export/usr/lib/x86_64-linux-gnu Rscript -e 'library(tidyverse, lib.loc="/export/usr/lib/R/site-library"); stopifnot(as.character(packageVersion("tidyverse", lib.loc="/export/usr/lib/R/site-library")) == "2.0.0")'; \
    Rscript -e 'stopifnot(grepl("^R version 4.5.3", R.version.string))'; \
    test -f /export/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf; \
    case "$(uname -m)" in x86_64) suffix=amd64; sum=5def6e1ff535e397becce292ee97767a947306150b9fb1488003b67ac3417c5e ;; aarch64) suffix=arm64; sum=ad5cf63fe0420388d9ec513f02d03e061477b786d11a328164dce8ad7387b8bd ;; *) echo "unsupported guest architecture: $(uname -m)" >&2; exit 1 ;; esac; \
    wget -nv --timeout=120 --tries=5 -O /tmp/pandoc.tgz "https://github.com/jgm/pandoc/releases/download/3.6.4/pandoc-3.6.4-linux-${suffix}.tar.gz"; \
    echo "$sum  /tmp/pandoc.tgz" | sha256sum -c -; \
    tar -xzf /tmp/pandoc.tgz -C /tmp; \
    "/tmp/pandoc-3.6.4/bin/pandoc" --version | grep -q 'pandoc 3.6.4'; \
    install -m 0755 "/tmp/pandoc-3.6.4/bin/pandoc" /export/usr/local/bin/pandoc
FROM r-base:4.5.3
# Runtime stage copies prebuilt r-* packages, pandoc, and fonts.
# Do not install gcc, gfortran, clang, git, or node into this image.
COPY --from=packages /export/ /
RUN set -eu; \
    if grep -E '^(gcc|g\+\+|gfortran|cpp|clang|git|nodejs|node)$|^(gcc|g\+\+|gfortran|cpp|clang)-[0-9]+$' /usr/local/share/a3s/new-packages.txt; then echo 'refusing to install a toolchain into the analysis image' >&2; exit 1; fi; \
    for bin in gcc g++ gfortran cpp clang git node nodejs cc c++; do rm -f "/usr/bin/$bin" "/usr/local/bin/$bin"; done; \
    rm -f /usr/bin/gcc-* /usr/bin/g++-* /usr/bin/gfortran-* /usr/bin/clang-* /usr/bin/*-linux-gnu-gcc* /usr/bin/*-linux-gnu-g++* /usr/bin/*-linux-gnu-gfortran* /usr/bin/*-linux-gnu-cpp*; \
    Rscript -e 'library(tidyverse); stopifnot(as.character(packageVersion("tidyverse")) == "2.0.0")'; \
    pandoc --version | grep -q 'pandoc 3.6.4'; \
    Rscript -e 'stopifnot(grepl("^R version 4.5.3", R.version.string))'; \
    test -f /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf; \
    if command -v gcc >/dev/null 2>&1 || command -v g++ >/dev/null 2>&1 || command -v gfortran >/dev/null 2>&1 || command -v clang >/dev/null 2>&1 || command -v git >/dev/null 2>&1 || command -v node >/dev/null 2>&1 || command -v nodejs >/dev/null 2>&1; then echo 'toolchain remains in the analysis image' >&2; exit 1; fi
