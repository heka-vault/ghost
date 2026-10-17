"""Local audio transport on ``sounddevice`` (PortAudio ships inside the wheel).

A port of Pipecat's ``LocalAudioTransport`` (BSD-2, Daily) that avoids the
``pyaudio`` compile step. PortAudio talks to PipeWire through its PulseAudio shim,
so the default devices are whatever PipeWire says: on the workstation, the
echo-cancelled source and the speakers.
"""

from __future__ import annotations

import asyncio
from concurrent.futures import ThreadPoolExecutor

from loguru import logger
from pipecat.frames.frames import InputAudioRawFrame, OutputAudioRawFrame, StartFrame
from pipecat.processors.frame_processor import FrameProcessor, FrameProcessorSetup
from pipecat.transports.base_input import BaseInputTransport
from pipecat.transports.base_output import BaseOutputTransport
from pipecat.transports.base_transport import BaseTransport, TransportParams


def _sd():
    # Imported lazily: loading sounddevice needs the PortAudio shared library,
    # which test runners and CI boxes without audio do not have.
    import sounddevice as sd

    return sd


class SoundDeviceTransportParams(TransportParams):
    input_device: int | str | None = None
    output_device: int | str | None = None


class SoundDeviceInputTransport(BaseInputTransport):
    _params: SoundDeviceTransportParams

    def __init__(self, params: SoundDeviceTransportParams) -> None:
        super().__init__(params)
        self._stream = None

    async def setup(self, setup: FrameProcessorSetup) -> None:
        await super().setup(setup)
        blocksize = int(self.sample_rate / 100) * 2  # 20 ms
        sd = _sd()
        self._stream = sd.RawInputStream(
            samplerate=self.sample_rate,
            blocksize=blocksize,
            device=self._params.input_device,
            channels=self._params.audio_in_channels,
            dtype="int16",
            callback=self._on_audio,
        )
        logger.info(
            f"audio in: {sd.query_devices(self._stream.device)['name']} @ {self.sample_rate}"
        )

    async def cleanup(self) -> None:
        await super().cleanup()
        if self._stream:
            self._stream.stop()
            self._stream.close()
            self._stream = None

    async def start(self, frame: StartFrame) -> None:
        await super().start(frame)
        if self._stream:
            self._stream.start()
            await self.set_transport_ready(frame)

    def _on_audio(self, indata, frames: int, time, status) -> None:
        if status:
            logger.warning(f"audio in: {status}")
        frame = InputAudioRawFrame(
            audio=bytes(indata),
            sample_rate=self.sample_rate,
            num_channels=self._params.audio_in_channels,
        )
        asyncio.run_coroutine_threadsafe(self.push_audio_frame(frame), self.get_event_loop())


class SoundDeviceOutputTransport(BaseOutputTransport):
    _params: SoundDeviceTransportParams

    def __init__(self, params: SoundDeviceTransportParams) -> None:
        super().__init__(params)
        self._stream = None
        self._executor = ThreadPoolExecutor(max_workers=1)

    async def setup(self, setup: FrameProcessorSetup) -> None:
        await super().setup(setup)
        sd = _sd()
        self._stream = sd.RawOutputStream(
            samplerate=self.sample_rate,
            device=self._params.output_device,
            channels=self._params.audio_out_channels,
            dtype="int16",
        )
        logger.info(
            f"audio out: {sd.query_devices(self._stream.device)['name']} @ {self.sample_rate}"
        )

    async def cleanup(self) -> None:
        try:
            await super().cleanup()
            if self._stream:
                self._stream.stop()
                self._stream.close()
                self._stream = None
        finally:
            self._executor.shutdown(wait=False)

    async def start(self, frame: StartFrame) -> None:
        await super().start(frame)
        if self._stream:
            self._stream.start()
            await self.set_transport_ready(frame)

    async def write_audio_frame(self, frame: OutputAudioRawFrame) -> bool:
        if not self._stream:
            return False
        await self.get_event_loop().run_in_executor(self._executor, self._stream.write, frame.audio)
        return True


class SoundDeviceTransport(BaseTransport):
    def __init__(self, params: SoundDeviceTransportParams) -> None:
        super().__init__()
        self._params = params
        self._input: SoundDeviceInputTransport | None = None
        self._output: SoundDeviceOutputTransport | None = None

    def input(self) -> FrameProcessor:
        if not self._input:
            self._input = SoundDeviceInputTransport(self._params)
        return self._input

    def output(self) -> FrameProcessor:
        if not self._output:
            self._output = SoundDeviceOutputTransport(self._params)
        return self._output
