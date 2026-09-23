//! Analysis image recipes derived from an environment lock.
//!
//! The published stage is the last `FROM`. Compilers may exist in an earlier
//! stage. They are not installed into the published stage.

use crate::{UseError, UseResult};

const RECIPE_HEADER: &str = "\
# Build input for an analysis image. Desktop must not fetch these packages.
# The published OCI digest, not this file, is the installable identity.
";

const PANDOC_AMD64_SHA256: &str =
    "5def6e1ff535e397becce292ee97767a947306150b9fb1488003b67ac3417c5e";
const PANDOC_ARM64_SHA256: &str =
    "ad5cf63fe0420388d9ec513f02d03e061477b786d11a328164dce8ad7387b8bd";

const R_RECIPE: &str = r#"# Packages stage is not published. It may inherit the base image toolchain.
# science-r-debs.tar is the snapshot closure staged beside this file.
# The guest does not apt-get or wget that closure: both fault on this filesystem.
# Compilers, git, and node are not installed. This is not conda.
# Version checks use Rscript: on a case-insensitive build rootfs, R is littler.
FROM r-base:__R_BASE__ AS packages
COPY science-r-debs.tar /tmp/science-r-debs.tar
RUN set -eu; \
    mkdir -p /tmp/a3s-debs /export/usr/local/bin /export/usr/local/share/a3s; \
    tar --warning=no-unknown-keyword -xf /tmp/science-r-debs.tar -C /tmp/a3s-debs; \
    test -n "$(find /tmp/a3s-debs -name 'r-cran-tidyverse___TIDYVERSE__*.deb' -print -quit)"; \
    fonts_deb=$(find /tmp/a3s-debs -name 'fonts-dejavu___FONTS__*.deb' -print -quit); \
    test -n "$fonts_deb"; \
    fonts_upstream=$(basename "$fonts_deb" | sed 's/^fonts-dejavu_//; s/[-+].*//'); \
    test "$fonts_upstream" = "__FONTS__"; \
    find /tmp/a3s-debs -name '*.deb' -exec basename {} \; | sed 's/_.*//' | sort > /export/usr/local/share/a3s/new-packages.txt; \
    if grep -E '^(gcc|g\+\+|gfortran|cpp|clang|git|nodejs|node)$|^(gcc|g\+\+|gfortran|cpp|clang)-[0-9]+$' /export/usr/local/share/a3s/new-packages.txt; then echo 'refusing to install a toolchain into the analysis image' >&2; exit 1; fi; \
    for deb in /tmp/a3s-debs/*.deb; do dpkg-deb -x "$deb" /export; done; \
    LD_LIBRARY_PATH=/export/usr/lib/aarch64-linux-gnu:/export/usr/lib/x86_64-linux-gnu Rscript -e 'library(tidyverse, lib.loc="/export/usr/lib/R/site-library"); stopifnot(as.character(packageVersion("tidyverse", lib.loc="/export/usr/lib/R/site-library")) == "__TIDYVERSE__")'; \
    Rscript -e 'stopifnot(grepl("^R version __R_BASE__", R.version.string))'; \
    test -f /export/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf; \
    case "$(uname -m)" in x86_64) suffix=amd64; sum=__PANDOC_AMD64__ ;; aarch64) suffix=arm64; sum=__PANDOC_ARM64__ ;; *) echo "unsupported guest architecture: $(uname -m)" >&2; exit 1 ;; esac; \
    wget -nv --timeout=120 --tries=5 -O /tmp/pandoc.tgz "https://github.com/jgm/pandoc/releases/download/__PANDOC__/pandoc-__PANDOC__-linux-${suffix}.tar.gz"; \
    echo "$sum  /tmp/pandoc.tgz" | sha256sum -c -; \
    tar -xzf /tmp/pandoc.tgz -C /tmp; \
    "/tmp/pandoc-__PANDOC__/bin/pandoc" --version | grep -q 'pandoc __PANDOC__'; \
    install -m 0755 "/tmp/pandoc-__PANDOC__/bin/pandoc" /export/usr/local/bin/pandoc
FROM r-base:__R_BASE__
# Runtime stage copies prebuilt r-* packages, pandoc, and fonts.
# Do not install gcc, gfortran, clang, git, or node into this image.
COPY --from=packages /export/ /
RUN set -eu; \
    if grep -E '^(gcc|g\+\+|gfortran|cpp|clang|git|nodejs|node)$|^(gcc|g\+\+|gfortran|cpp|clang)-[0-9]+$' /usr/local/share/a3s/new-packages.txt; then echo 'refusing to install a toolchain into the analysis image' >&2; exit 1; fi; \
    for bin in gcc g++ gfortran cpp clang git node nodejs cc c++; do rm -f "/usr/bin/$bin" "/usr/local/bin/$bin"; done; \
    rm -f /usr/bin/gcc-* /usr/bin/g++-* /usr/bin/gfortran-* /usr/bin/clang-* /usr/bin/*-linux-gnu-gcc* /usr/bin/*-linux-gnu-g++* /usr/bin/*-linux-gnu-gfortran* /usr/bin/*-linux-gnu-cpp*; \
    Rscript -e 'library(tidyverse); stopifnot(as.character(packageVersion("tidyverse")) == "__TIDYVERSE__")'; \
    pandoc --version | grep -q 'pandoc __PANDOC__'; \
    Rscript -e 'stopifnot(grepl("^R version __R_BASE__", R.version.string))'; \
    test -f /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf; \
    if command -v gcc >/dev/null 2>&1 || command -v g++ >/dev/null 2>&1 || command -v gfortran >/dev/null 2>&1 || command -v clang >/dev/null 2>&1 || command -v git >/dev/null 2>&1 || command -v node >/dev/null 2>&1 || command -v nodejs >/dev/null 2>&1; then echo 'toolchain remains in the analysis image' >&2; exit 1; fi
"#;

pub(crate) fn render_python_runtime_recipe(packages: &[(String, String)]) -> UseResult<String> {
    for (name, version) in packages {
        recipe_token(name)?;
        recipe_token(version)?;
    }
    let mut recipe = String::from(RECIPE_HEADER);
    recipe.push_str("FROM python:3.11.12-slim-bookworm\n");
    recipe.push_str("RUN python -m pip install --no-cache-dir --timeout 300 --retries 10");
    for (name, version) in packages {
        recipe.push_str(&format!(" \\\n    {name}=={version}"));
    }
    recipe.push('\n');
    Ok(recipe)
}

/// Two-stage R recipe. The packages stage copies a staged Debian snapshot
/// archive and extracts those debs. The last stage copies them onto `r-base`
/// and removes the base image toolchain from `PATH`.
pub(crate) fn render_r_runtime_recipe(
    r_base: &str,
    tidyverse: &str,
    pandoc: &str,
    fonts: &str,
) -> UseResult<String> {
    recipe_token(r_base)?;
    recipe_token(tidyverse)?;
    recipe_token(pandoc)?;
    recipe_token(fonts)?;
    let body = R_RECIPE
        .replace("__R_BASE__", r_base)
        .replace("__TIDYVERSE__", tidyverse)
        .replace("__PANDOC_AMD64__", PANDOC_AMD64_SHA256)
        .replace("__PANDOC_ARM64__", PANDOC_ARM64_SHA256)
        .replace("__PANDOC__", pandoc)
        .replace("__FONTS__", fonts);
    if body.contains("__") {
        return Err(UseError::new(
            "environment.lock.recipe",
            "R image recipe still contains an unsubstituted pin.",
        ));
    }
    Ok(format!("{RECIPE_HEADER}{body}"))
}

fn recipe_token(value: &str) -> UseResult<()> {
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
    {
        return Err(UseError::new(
            "environment.lock.recipe",
            format!("Package pin '{value}' cannot be embedded in an image recipe."),
        ));
    }
    Ok(())
}
