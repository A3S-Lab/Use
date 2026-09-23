# Build input for an analysis image. Desktop must not fetch these packages.
# The published OCI digest, not this file, is the installable identity.
FROM python:3.11.12-slim-bookworm
RUN python -m pip install --no-cache-dir --timeout 300 --retries 10 \
    matplotlib==3.10.1 \
    numpy==2.2.6 \
    pandas==2.2.3 \
    pillow==11.1.0 \
    scikit-learn==1.6.1 \
    scipy==1.15.2 \
    seaborn==0.13.2 \
    statsmodels==0.14.4
