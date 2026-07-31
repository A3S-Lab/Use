def attest(receipt: dict[str, object]) -> bool:
    return "run_id" in receipt and "result" in receipt
