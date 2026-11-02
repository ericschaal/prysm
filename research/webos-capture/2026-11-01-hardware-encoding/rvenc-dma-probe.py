import ctypes as C
import fcntl
import mmap
import os
import struct
import sys
import time

def api(library, name, result, *args):
    function = getattr(library, name)
    function.restype = result
    function.argtypes = args
    return function

gst = C.CDLL('libgstreamer-1.0.so.0')
app = C.CDLL('libgstapp-1.0.so.0')
video = C.CDLL('libgstvideo-1.0.so.0')
alloc = C.CDLL('libgstallocators-1.0.so.0')
p, u, i, s = C.c_void_p, C.c_uint, C.c_int, C.c_char_p
api(gst, 'gst_init', None, p, p)(None, None)
parse = api(gst, 'gst_parse_launch', p, s, p)
lookup = api(gst, 'gst_bin_get_by_name', p, p, s)
state = api(gst, 'gst_element_set_state', i, p, i)
unref = api(gst, 'gst_object_unref', None, p)
new_buffer = api(gst, 'gst_buffer_new', p)
append = api(gst, 'gst_buffer_append_memory', None, p, p)
new_allocator = api(alloc, 'gst_dmabuf_allocator_new', p)
allocate = api(alloc, 'gst_dmabuf_allocator_alloc', p, p, i, C.c_size_t)
pixel_format = api(video, 'gst_video_format_from_string', i, s)(b'NV12')
add_meta = api(video, 'gst_buffer_add_video_meta', p, p, i, i, u, u)
push = api(app, 'gst_app_src_push_buffer', i, p, p)
eos = api(app, 'gst_app_src_end_of_stream', i, p)
get_bus = api(gst, 'gst_element_get_bus', p, p)
poll = api(gst, 'gst_bus_timed_pop_filtered', p, p, C.c_uint64, u)
unref_message = api(gst, 'gst_mini_object_unref', None, p)

width, height = 640, 360
size = width * height * 3 // 2
destination = sys.argv[1]
heap_path = sys.argv[2] if len(sys.argv) > 2 else '/dev/dma_heap/dvr'
pipeline = parse(('appsrc name=input is-live=true do-timestamp=true format=time '
    'caps="video/x-raw(memory:DMABuf),format=NV12,width=640,height=360,framerate=15/1,colorimetry=bt709" '
    '! rvenc target-bitrate=2000000 ! h264parse config-interval=-1 '
    '! video/x-h264,stream-format=byte-stream,alignment=au '
    '! filesink location=' + destination).encode(), None)
assert pipeline, 'pipeline construction failed'
source = lookup(pipeline, b'input')
allocator = new_allocator()
bus = get_bus(pipeline)
heap = os.open(heap_path, os.O_RDWR | os.O_CLOEXEC)
try:
    assert state(pipeline, 4) != 0, 'PLAYING failed'
    for index in range(15):
        request = bytearray(struct.pack('QIIQ', size, 0, os.O_RDWR | os.O_CLOEXEC, 0))
        fcntl.ioctl(heap, 0xc0184800, request, True)
        dma_fd = struct.unpack('QIIQ', request)[1]
        with mmap.mmap(dma_fd, size) as pixels:
            fcntl.ioctl(dma_fd, 0x40086200, struct.pack('Q', 2))
            pixels[:] = bytes([32 + index * 8]) * (width * height) + bytes([128]) * (size - width * height)
            fcntl.ioctl(dma_fd, 0x40086200, struct.pack('Q', 6))
        memory = allocate(allocator, dma_fd, size)
        assert memory, 'DMABuf wrapping failed'
        buffer = new_buffer()
        append(buffer, memory)
        assert add_meta(buffer, 0, pixel_format, width, height), 'video metadata failed'
        result = push(source, buffer)
        print('push', index, result, flush=True)
        assert result == 0, 'input buffer rejected'
        time.sleep(1 / 15)
    assert eos(source) == 0, 'EOS rejected'
    message = poll(bus, 4_000_000_000, 1)
    assert message, 'no EOS before timeout'
    unref_message(message)
finally:
    state(pipeline, 1)
    os.close(heap)
    for obj in (bus, source, allocator, pipeline):
        unref(obj)
assert os.path.getsize(destination) > 0, 'encoder produced no output'
print('OUTPUT_BYTES', os.path.getsize(destination))
