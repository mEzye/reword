# Speech model and runtime

Reword can download and run Parakeet TDT 0.6B v3 UK, version 3.

- Fine-tuned model: Yuri Ihnatov, https://huggingface.co/ihnatov-yuri/parakeet-tdt-0.6b-v3-uk
- Base model: NVIDIA Parakeet TDT 0.6B v3, https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
- Model license: Creative Commons Attribution 4.0, https://creativecommons.org/licenses/by/4.0/
- Q8_0 GGUF conversion: vladkens, https://huggingface.co/vladkens/parakeet-tdt-0.6b-v3-uk-gguf
- Conversion revision: df469cd0d1944c4368bef64a37c7f0393d00b42c
- Model SHA-256: 7645a16ed64f5ceb12e6b4a3cd82558dbbc7e1d505842d08a534ecfdfa471d2d

Credit: Yuri Ihnatov, fine-tuned from NVIDIA Parakeet TDT 0.6B v3. Converted to GGUF and quantized to Q8_0 by vladkens. Reword does not modify the downloaded model weights. See the original model card for training-data provenance and limitations.

Native inference uses transcribe.cpp 0.4.2, https://github.com/handy-computer/transcribe.cpp (MIT). The downloaded native bundle retains its `licenses` directory, including the ggml and vendored-component notices.

Models and native inference files are downloaded separately, not committed to the Reword repository. `setup-speech.ps1` pins versions and verifies SHA-256 hashes.
