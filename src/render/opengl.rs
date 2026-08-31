//! OpenGL 3.2 core-profile presentation backend (ports `render_ogl.c`).
//!
//! Builds a WGL_ARB_create_context 3.2 core context, loads the non-1.1 GL
//! entry points through `wglGetProcAddress` and renders the DDraw primary with
//! a GLSL shader pipeline (built-in resampling filters or external
//! libretro/cnc-ddraw-style `.glsl` files). VSync and `SwapBuffers` are
//! handled by the caller in `render/mod.rs`; the context is left current for
//! that.

use std::ffi::CStr;
use std::sync::atomic::Ordering;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::OpenGL::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::PCSTR;

use crate::render::scale;
use crate::state::{SurfaceBuffers, state};

const GL_BGRA: u32 = 0x80E1;
const GL_UNSIGNED_SHORT_5_6_5: u32 = 33635;
const GL_UNSIGNED_SHORT_1_5_5_5_REV: u32 = 32822;
const GL_TEXTURE_MAX_LEVEL: u32 = 33117;
const GL_CLAMP_TO_EDGE: u32 = 0x812F;
const GL_ARRAY_BUFFER: u32 = 0x8892;
const GL_STATIC_DRAW: u32 = 0x88E4;
const GL_VERTEX_SHADER: u32 = 0x8B31;
const GL_FRAGMENT_SHADER: u32 = 0x8B30;
const GL_COMPILE_STATUS: u32 = 0x8B81;
const GL_LINK_STATUS: u32 = 0x8B82;
const GL_INFO_LOG_LENGTH: u32 = 0x8B84;
const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_RENDERBUFFER: u32 = 0x8D41;
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_TEXTURE0: u32 = 0x84C0;
const GL_RGB565: u32 = 0x8D62;
const GL_SYNC_GPU_COMMANDS_COMPLETE: u32 = 0x9117;
const GL_SYNC_FLUSH_COMMANDS_BIT: u32 = 0x00000001;

const WGL_DRAW_TO_WINDOW_ARB: i32 = 0x2001;
const WGL_SUPPORT_OPENGL_ARB: i32 = 0x2010;
const WGL_DOUBLE_BUFFER_ARB: i32 = 0x2011;
const WGL_PIXEL_TYPE_ARB: i32 = 0x2013;
const WGL_TYPE_RGBA_ARB: i32 = 0x202B;
const WGL_COLOR_BITS_ARB: i32 = 0x2014;
const WGL_CONTEXT_MAJOR_VERSION_ARB: i32 = 0x2091;
const WGL_CONTEXT_MINOR_VERSION_ARB: i32 = 0x2092;
const WGL_CONTEXT_FLAGS_ARB: i32 = 0x2094;
const WGL_CONTEXT_FORWARD_COMPATIBLE_BIT_ARB: i32 = 0x00000002;
const WGL_CONTEXT_PROFILE_MASK_ARB: i32 = 0x9126;
const WGL_CONTEXT_CORE_PROFILE_BIT_ARB: i32 = 0x00000001;

type CreateContextAttribsFn = unsafe extern "system" fn(HDC, HGLRC, *const i32) -> HGLRC;
type ChoosePixelFormatArbFn = unsafe extern "system" fn(HDC, *const i32, *const f32, u32, *mut i32, *mut u32) -> i32;
type SwapIntervalFn = unsafe extern "system" fn(i32) -> i32;

type GenBuffersFn = unsafe extern "system" fn(i32, *mut u32);
type DeleteBuffersFn = unsafe extern "system" fn(i32, *const u32);
type BindBufferFn = unsafe extern "system" fn(u32, u32);
type BufferDataFn = unsafe extern "system" fn(u32, isize, *const core::ffi::c_void, u32);
type GenVertexArraysFn = unsafe extern "system" fn(i32, *mut u32);
type DeleteVertexArraysFn = unsafe extern "system" fn(i32, *const u32);
type BindVertexArrayFn = unsafe extern "system" fn(u32);
type VertexAttribPointerFn = unsafe extern "system" fn(u32, i32, u32, u8, i32, *const core::ffi::c_void);
type EnableVertexAttribArrayFn = unsafe extern "system" fn(u32);
type DisableVertexAttribArrayFn = unsafe extern "system" fn(u32);
type VertexAttrib4fFn = unsafe extern "system" fn(u32, f32, f32, f32, f32);
type DrawArraysFn = unsafe extern "system" fn(u32, i32, i32);
type CreateShaderFn = unsafe extern "system" fn(u32) -> u32;
type DeleteShaderFn = unsafe extern "system" fn(u32);
type ShaderSourceFn = unsafe extern "system" fn(u32, i32, *const *const i8, *const i32);
type CompileShaderFn = unsafe extern "system" fn(u32);
type GetShaderivFn = unsafe extern "system" fn(u32, u32, *mut i32);
type GetShaderInfoLogFn = unsafe extern "system" fn(u32, i32, *mut i32, *mut i8);
type CreateProgramFn = unsafe extern "system" fn() -> u32;
type DeleteProgramFn = unsafe extern "system" fn(u32);
type UseProgramFn = unsafe extern "system" fn(u32);
type AttachShaderFn = unsafe extern "system" fn(u32, u32);
type DetachShaderFn = unsafe extern "system" fn(u32, u32);
type LinkProgramFn = unsafe extern "system" fn(u32);
type GetProgramivFn = unsafe extern "system" fn(u32, u32, *mut i32);
type GetProgramInfoLogFn = unsafe extern "system" fn(u32, i32, *mut i32, *mut i8);
type GetUniformLocationFn = unsafe extern "system" fn(u32, *const i8) -> i32;
type Uniform1iFn = unsafe extern "system" fn(i32, i32);
type Uniform2fFn = unsafe extern "system" fn(i32, f32, f32);
type UniformMatrix4fvFn = unsafe extern "system" fn(i32, i32, u8, *const f32);
type GetAttribLocationFn = unsafe extern "system" fn(u32, *const i8) -> i32;
type GenTexturesFn = unsafe extern "system" fn(i32, *mut u32);
type DeleteTexturesFn = unsafe extern "system" fn(i32, *const u32);
type BindTextureFn = unsafe extern "system" fn(u32, u32);
type TexImage2dFn = unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const core::ffi::c_void);
type TexSubImage2dFn = unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const core::ffi::c_void);
type TexParameteriFn = unsafe extern "system" fn(u32, u32, i32);
type ActiveTextureFn = unsafe extern "system" fn(u32);
type GenFramebuffersFn = unsafe extern "system" fn(i32, *mut u32);
type DeleteFramebuffersFn = unsafe extern "system" fn(i32, *const u32);
type BindFramebufferFn = unsafe extern "system" fn(u32, u32);
type FramebufferTexture2dFn = unsafe extern "system" fn(u32, u32, u32, u32, i32);
type CheckFramebufferStatusFn = unsafe extern "system" fn(u32) -> u32;
type BlendFuncFn = unsafe extern "system" fn(u32, u32);
type GenRenderbuffersFn = unsafe extern "system" fn(i32, *mut u32);
type BindRenderbufferFn = unsafe extern "system" fn(u32, u32);
type RenderbufferStorageFn = unsafe extern "system" fn(u32, u32, i32, i32);
type FramebufferRenderbufferFn = unsafe extern "system" fn(u32, u32, u32, u32);
type FenceSyncFn = unsafe extern "system" fn(u32, u32) -> *mut core::ffi::c_void;
type ClientWaitSyncFn = unsafe extern "system" fn(*mut core::ffi::c_void, u32, u64) -> u32;
type DeleteSyncFn = unsafe extern "system" fn(*mut core::ffi::c_void);

struct Gl {
    gen_buffers: GenBuffersFn,
    delete_buffers: DeleteBuffersFn,
    bind_buffer: BindBufferFn,
    buffer_data: BufferDataFn,
    gen_vertex_arrays: GenVertexArraysFn,
    delete_vertex_arrays: DeleteVertexArraysFn,
    bind_vertex_array: BindVertexArrayFn,
    vertex_attrib_pointer: VertexAttribPointerFn,
    enable_vertex_attrib_array: EnableVertexAttribArrayFn,
    disable_vertex_attrib_array: DisableVertexAttribArrayFn,
    vertex_attrib4f: VertexAttrib4fFn,
    draw_arrays: DrawArraysFn,
    create_shader: CreateShaderFn,
    delete_shader: DeleteShaderFn,
    shader_source: ShaderSourceFn,
    compile_shader: CompileShaderFn,
    get_shaderiv: GetShaderivFn,
    get_shader_info_log: GetShaderInfoLogFn,
    create_program: CreateProgramFn,
    delete_program: DeleteProgramFn,
    use_program: UseProgramFn,
    link_program: LinkProgramFn,
    attach_shader: AttachShaderFn,
    detach_shader: DetachShaderFn,
    get_programiv: GetProgramivFn,
    get_program_info_log: GetProgramInfoLogFn,
    get_uniform_location: GetUniformLocationFn,
    uniform_1i: Uniform1iFn,
    uniform_2f: Uniform2fFn,
    uniform_matrix4fv: UniformMatrix4fvFn,
    get_attrib_location: GetAttribLocationFn,
    gen_textures: GenTexturesFn,
    delete_textures: DeleteTexturesFn,
    bind_texture: BindTextureFn,
    tex_image_2d: TexImage2dFn,
    tex_sub_image_2d: TexSubImage2dFn,
    tex_parameteri: TexParameteriFn,
    active_texture: ActiveTextureFn,
    gen_framebuffers: GenFramebuffersFn,
    delete_framebuffers: DeleteFramebuffersFn,
    bind_framebuffer: BindFramebufferFn,
    framebuffer_texture_2d: FramebufferTexture2dFn,
    check_framebuffer_status: CheckFramebufferStatusFn,
    blend_func: BlendFuncFn,
    gen_renderbuffers: GenRenderbuffersFn,
    bind_renderbuffer: BindRenderbufferFn,
    renderbuffer_storage: RenderbufferStorageFn,
    framebuffer_renderbuffer: FramebufferRenderbufferFn,
    fence_sync: FenceSyncFn,
    client_wait_sync: ClientWaitSyncFn,
    delete_sync: DeleteSyncFn,
}

unsafe fn load_proc<T>(name: &[u8]) -> Option<T> {
    let addr: PROC = wglGetProcAddress(PCSTR(name.as_ptr()));
    if addr.is_some() { Some(std::mem::transmute_copy::<PROC, T>(&addr)) } else { None }
}

impl Gl {
    unsafe fn load() -> Option<Gl> {
        macro_rules! req {
            ($name:literal) => {{
                match load_proc(concat!($name, "\0").as_bytes()) {
                    Some(p) => p,
                    None => {
                        crate::dd_log!("opengl: missing GL proc {}", $name);
                        return None;
                    }
                }
            }};
        }
        Some(Gl {
            gen_buffers: req!("glGenBuffers"),
            delete_buffers: req!("glDeleteBuffers"),
            bind_buffer: req!("glBindBuffer"),
            buffer_data: req!("glBufferData"),
            gen_vertex_arrays: req!("glGenVertexArrays"),
            delete_vertex_arrays: req!("glDeleteVertexArrays"),
            bind_vertex_array: req!("glBindVertexArray"),
            vertex_attrib_pointer: req!("glVertexAttribPointer"),
            enable_vertex_attrib_array: req!("glEnableVertexAttribArray"),
            disable_vertex_attrib_array: req!("glDisableVertexAttribArray"),
            vertex_attrib4f: req!("glVertexAttrib4f"),
            draw_arrays: req!("glDrawArrays"),
            create_shader: req!("glCreateShader"),
            delete_shader: req!("glDeleteShader"),
            shader_source: req!("glShaderSource"),
            compile_shader: req!("glCompileShader"),
            get_shaderiv: req!("glGetShaderiv"),
            get_shader_info_log: req!("glGetShaderInfoLog"),
            create_program: req!("glCreateProgram"),
            delete_program: req!("glDeleteProgram"),
            use_program: req!("glUseProgram"),
            link_program: req!("glLinkProgram"),
            attach_shader: req!("glAttachShader"),
            detach_shader: req!("glDetachShader"),
            get_programiv: req!("glGetProgramiv"),
            get_program_info_log: req!("glGetProgramInfoLog"),
            get_uniform_location: req!("glGetUniformLocation"),
            uniform_1i: req!("glUniform1i"),
            uniform_2f: req!("glUniform2f"),
            uniform_matrix4fv: req!("glUniformMatrix4fv"),
            get_attrib_location: req!("glGetAttribLocation"),
            gen_textures: req!("glGenTextures"),
            delete_textures: req!("glDeleteTextures"),
            bind_texture: req!("glBindTexture"),
            tex_image_2d: req!("glTexImage2D"),
            tex_sub_image_2d: req!("glTexSubImage2D"),
            tex_parameteri: req!("glTexParameteri"),
            active_texture: req!("glActiveTexture"),
            gen_framebuffers: req!("glGenFramebuffers"),
            delete_framebuffers: req!("glDeleteFramebuffers"),
            bind_framebuffer: req!("glBindFramebuffer"),
            framebuffer_texture_2d: req!("glFramebufferTexture2D"),
            check_framebuffer_status: req!("glCheckFramebufferStatus"),
            blend_func: req!("glBlendFunc"),
            gen_renderbuffers: req!("glGenRenderbuffers"),
            bind_renderbuffer: req!("glBindRenderbuffer"),
            renderbuffer_storage: req!("glRenderbufferStorage"),
            framebuffer_renderbuffer: req!("glFramebufferRenderbuffer"),
            fence_sync: req!("glFenceSync"),
            client_wait_sync: req!("glClientWaitSync"),
            delete_sync: req!("glDeleteSync"),
        })
    }
}

struct BoundProgram {
    id: u32,
    pos: i32,
    uv: i32,
    color: i32,
}

enum ShaderRef {
    Builtin(&'static str),
    File(String),
}

impl ShaderRef {
    fn key(&self) -> String {
        match self {
            ShaderRef::Builtin(n) => format!("builtin:{}", n),
            ShaderRef::File(p) => format!("file:{}", p),
        }
    }
}

const FIXED_VERT: &str = "#version 150\n\
    in vec4 VertexCoord;\n\
    in vec4 Color;\n\
    in vec2 TexCoord;\n\
    out vec4 TEX0;\n\
    out vec4 COL0;\n\
    out vec2 tex_coord;\n\
    uniform mat4 MVPMatrix;\n\
    void main()\n\
    {\n\
        gl_Position = MVPMatrix * VertexCoord;\n\
        TEX0 = vec4(TexCoord, 0.0, 1.0);\n\
        COL0 = Color;\n\
        tex_coord = TexCoord;\n\
    }\n";

const NEAREST_FRAG: &str = "#version 150\n\
    out vec4 FragColor;\n\
    uniform sampler2D Texture;\n\
    in vec4 TEX0;\n\
    void main()\n\
    {\n\
        FragColor = texture(Texture, TEX0.xy);\n\
    }\n";

const BILINEAR_FRAG: &str = "#version 150\n\
    out vec4 FragColor;\n\
    uniform sampler2D Texture;\n\
    uniform vec2 TextureSize;\n\
    in vec4 TEX0;\n\
    void main()\n\
    {\n\
        vec2 pos = TEX0.xy * TextureSize - vec2(0.5);\n\
        vec2 f = fract(pos);\n\
        vec2 base = (floor(pos) + vec2(0.5)) / TextureSize;\n\
        vec2 s = vec2(1.0) / TextureSize;\n\
        vec4 tl = texture(Texture, base);\n\
        vec4 tr = texture(Texture, base + vec2(s.x, 0.0));\n\
        vec4 bl = texture(Texture, base + vec2(0.0, s.y));\n\
        vec4 br = texture(Texture, base + s);\n\
        FragColor = mix(mix(tl, tr, f.x), mix(bl, br, f.x), f.y);\n\
    }\n";

const CATMULL_ROM_FRAG: &str = "#version 150\n\
    out vec4 FragColor;\n\
    uniform int FrameDirection;\n\
    uniform int FrameCount;\n\
    uniform vec2 OutputSize;\n\
    uniform vec2 TextureSize;\n\
    uniform vec2 InputSize;\n\
    uniform sampler2D Texture;\n\
    in vec4 TEX0;\n\
    #define SourceSize vec4(TextureSize, 1.0 / TextureSize)\n\
    void main()\n\
    {\n\
        vec2 samplePos = TEX0.xy * SourceSize.xy;\n\
        vec2 texPos1 = floor(samplePos - 0.5) + 0.5;\n\
        vec2 f = samplePos - texPos1;\n\
        vec2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));\n\
        vec2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);\n\
        vec2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));\n\
        vec2 w3 = f * f * (-0.5 + 0.5 * f);\n\
        vec2 w12 = w1 + w2;\n\
        vec2 offset12 = w2 / (w1 + w2);\n\
        vec2 texPos0 = texPos1 - 1.0;\n\
        vec2 texPos3 = texPos1 + 2.0;\n\
        vec2 texPos12 = texPos1 + offset12;\n\
        texPos0 *= SourceSize.zw;\n\
        texPos3 *= SourceSize.zw;\n\
        texPos12 *= SourceSize.zw;\n\
        float wtm = w12.x * w0.y;\n\
        float wml = w0.x * w12.y;\n\
        float wmm = w12.x * w12.y;\n\
        float wmr = w3.x * w12.y;\n\
        float wbm = w12.x * w3.y;\n\
        vec3 result = vec3(0.0);\n\
        result += texture(Texture, vec2(texPos12.x, texPos0.y)).rgb * wtm;\n\
        result += texture(Texture, vec2(texPos0.x, texPos12.y)).rgb * wml;\n\
        result += texture(Texture, vec2(texPos12.x, texPos12.y)).rgb * wmm;\n\
        result += texture(Texture, vec2(texPos3.x, texPos12.y)).rgb * wmr;\n\
        result += texture(Texture, vec2(texPos12.x, texPos3.y)).rgb * wbm;\n\
        FragColor = vec4(result * (1.0 / (wtm + wml + wmm + wmr + wbm)), 1.0);\n\
    }\n";

const LANCZOS2_FRAG: &str = "#version 150\n\
    #define JINC2_WINDOW_SINC 0.5\n\
    #define JINC2_SINC 1.0\n\
    #define JINC2_AR_STRENGTH 0.8\n\
    out vec4 FragColor;\n\
    uniform int FrameDirection;\n\
    uniform int FrameCount;\n\
    uniform vec2 OutputSize;\n\
    uniform vec2 TextureSize;\n\
    uniform vec2 InputSize;\n\
    uniform sampler2D Texture;\n\
    in vec4 TEX0;\n\
    const float pi = 3.1415926535897932384626433832795;\n\
    const float wa = JINC2_WINDOW_SINC * pi;\n\
    const float wb = JINC2_SINC * pi;\n\
    float d(vec2 pt1, vec2 pt2)\n\
    {\n\
        vec2 v = pt2 - pt1;\n\
        return sqrt(dot(v, v));\n\
    }\n\
    vec3 min4(vec3 a, vec3 b, vec3 c, vec3 d)\n\
    {\n\
        return min(a, min(b, min(c, d)));\n\
    }\n\
    vec3 max4(vec3 a, vec3 b, vec3 c, vec3 d)\n\
    {\n\
        return max(a, max(b, max(c, d)));\n\
    }\n\
    vec4 resampler(vec4 x)\n\
    {\n\
        vec4 res;\n\
        res.x = (x.x == 0.0) ? wa * wb : sin(x.x * wa) * sin(x.x * wb) / (x.x * x.x);\n\
        res.y = (x.y == 0.0) ? wa * wb : sin(x.y * wa) * sin(x.y * wb) / (x.y * x.y);\n\
        res.z = (x.z == 0.0) ? wa * wb : sin(x.z * wa) * sin(x.z * wb) / (x.z * x.z);\n\
        res.w = (x.w == 0.0) ? wa * wb : sin(x.w * wa) * sin(x.w * wb) / (x.w * x.w);\n\
        return res;\n\
    }\n\
    void main()\n\
    {\n\
        vec3 color;\n\
        vec4 weights[4];\n\
        vec2 dx = vec2(1.0, 0.0);\n\
        vec2 dy = vec2(0.0, 1.0);\n\
        vec2 pc = TEX0.xy * TextureSize;\n\
        vec2 tc = floor(pc - vec2(0.5, 0.5)) + vec2(0.5, 0.5);\n\
        weights[0] = resampler(vec4(d(pc, tc - dx - dy), d(pc, tc - dy), d(pc, tc + dx - dy), d(pc, tc + 2.0 * dx - dy)));\n\
        weights[1] = resampler(vec4(d(pc, tc - dx), d(pc, tc), d(pc, tc + dx), d(pc, tc + 2.0 * dx)));\n\
        weights[2] = resampler(vec4(d(pc, tc - dx + dy), d(pc, tc + dy), d(pc, tc + dx + dy), d(pc, tc + 2.0 * dx + dy)));\n\
        weights[3] = resampler(vec4(d(pc, tc - dx + 2.0 * dy), d(pc, tc + 2.0 * dy), d(pc, tc + dx + 2.0 * dy), d(pc, tc + 2.0 * dx + 2.0 * dy)));\n\
        dx = dx / TextureSize;\n\
        dy = dy / TextureSize;\n\
        tc = tc / TextureSize;\n\
        vec3 c00 = texture(Texture, tc - dx - dy).xyz;\n\
        vec3 c10 = texture(Texture, tc - dy).xyz;\n\
        vec3 c20 = texture(Texture, tc + dx - dy).xyz;\n\
        vec3 c30 = texture(Texture, tc + 2.0 * dx - dy).xyz;\n\
        vec3 c01 = texture(Texture, tc - dx).xyz;\n\
        vec3 c11 = texture(Texture, tc).xyz;\n\
        vec3 c21 = texture(Texture, tc + dx).xyz;\n\
        vec3 c31 = texture(Texture, tc + 2.0 * dx).xyz;\n\
        vec3 c02 = texture(Texture, tc - dx + dy).xyz;\n\
        vec3 c12 = texture(Texture, tc + dy).xyz;\n\
        vec3 c22 = texture(Texture, tc + dx + dy).xyz;\n\
        vec3 c32 = texture(Texture, tc + 2.0 * dx + dy).xyz;\n\
        vec3 c03 = texture(Texture, tc - dx + 2.0 * dy).xyz;\n\
        vec3 c13 = texture(Texture, tc + 2.0 * dy).xyz;\n\
        vec3 c23 = texture(Texture, tc + dx + 2.0 * dy).xyz;\n\
        vec3 c33 = texture(Texture, tc + 2.0 * dx + 2.0 * dy).xyz;\n\
        vec3 min_sample = min4(c11, c21, c12, c22);\n\
        vec3 max_sample = max4(c11, c21, c12, c22);\n\
        color = vec3(dot(weights[0], vec4(c00.x, c10.x, c20.x, c30.x)), dot(weights[0], vec4(c00.y, c10.y, c20.y, c30.y)), dot(weights[0], vec4(c00.z, c10.z, c20.z, c30.z)));\n\
        color += vec3(dot(weights[1], vec4(c01.x, c11.x, c21.x, c31.x)), dot(weights[1], vec4(c01.y, c11.y, c21.y, c31.y)), dot(weights[1], vec4(c01.z, c11.z, c21.z, c31.z)));\n\
        color += vec3(dot(weights[2], vec4(c02.x, c12.x, c22.x, c32.x)), dot(weights[2], vec4(c02.y, c12.y, c22.y, c32.y)), dot(weights[2], vec4(c02.z, c12.z, c22.z, c32.z)));\n\
        color += vec3(dot(weights[3], vec4(c03.x, c13.x, c23.x, c33.x)), dot(weights[3], vec4(c03.y, c13.y, c23.y, c33.y)), dot(weights[3], vec4(c03.z, c13.z, c23.z, c33.z)));\n\
        color = color / (dot(weights[0], vec4(1.0, 1.0, 1.0, 1.0)) + dot(weights[1], vec4(1.0, 1.0, 1.0, 1.0)) + dot(weights[2], vec4(1.0, 1.0, 1.0, 1.0)) + dot(weights[3], vec4(1.0, 1.0, 1.0, 1.0)));\n\
        vec3 aux = color;\n\
        color = clamp(color, min_sample, max_sample);\n\
        color = mix(aux, color, JINC2_AR_STRENGTH);\n\
        FragColor.xyz = color;\n\
        FragColor.w = 1.0;\n\
    }\n";

const XBR_LV2_VERT: &str = "#version 150\n\
    #define texCoord TEX0\n\
    #define t1 TEX1\n\
    #define t2 TEX2\n\
    #define t3 TEX3\n\
    #define t4 TEX4\n\
    #define t5 TEX5\n\
    #define t6 TEX6\n\
    #define t7 TEX7\n\
    in vec4 VertexCoord;\n\
    in vec4 Color;\n\
    in vec2 TexCoord;\n\
    out vec4 color;\n\
    out vec2 texCoord;\n\
    out vec4 t1;\n\
    out vec4 t2;\n\
    out vec4 t3;\n\
    out vec4 t4;\n\
    out vec4 t5;\n\
    out vec4 t6;\n\
    out vec4 t7;\n\
    uniform mat4 MVPMatrix;\n\
    uniform int FrameDirection;\n\
    uniform int FrameCount;\n\
    uniform vec2 OutputSize;\n\
    uniform vec2 TextureSize;\n\
    uniform vec2 InputSize;\n\
    void main()\n\
    {\n\
        gl_Position = MVPMatrix * VertexCoord;\n\
        color = Color;\n\
        float dx = (1.0 / TextureSize.x);\n\
        float dy = (1.0 / TextureSize.y);\n\
        texCoord = TexCoord;\n\
        texCoord.x *= 1.00000001;\n\
        t1 = TexCoord.xxxy + vec4(-dx, 0.0, dx, -2.0 * dy);\n\
        t2 = TexCoord.xxxy + vec4(-dx, 0.0, dx, -dy);\n\
        t3 = TexCoord.xxxy + vec4(-dx, 0.0, dx, 0.0);\n\
        t4 = TexCoord.xxxy + vec4(-dx, 0.0, dx, dy);\n\
        t5 = TexCoord.xxxy + vec4(-dx, 0.0, dx, 2.0 * dy);\n\
        t6 = TexCoord.xyyy + vec4(-2.0 * dx, -dy, 0.0, dy);\n\
        t7 = TexCoord.xyyy + vec4(2.0 * dx, -dy, 0.0, dy);\n\
    }\n";

const XBR_LV2_FRAG: &str = "#version 150\n\
    #define mul(a,b) (b*a)\n\
    #define CORNER_C\n\
    #define SMOOTH_TIPS\n\
    #define XBR_SCALE 3.0\n\
    #define lv2_cf 2.0\n\
    #define XBR_Y_WEIGHT 48.0\n\
    #define XBR_EQ_THRESHOLD 15.0\n\
    #define XBR_LV1_COEFFICIENT 0.5\n\
    #define XBR_LV2_COEFFICIENT 2.0\n\
    #define small_details 0.0\n\
    #define texCoord TEX0\n\
    #define t1 TEX1\n\
    #define t2 TEX2\n\
    #define t3 TEX3\n\
    #define t4 TEX4\n\
    #define t5 TEX5\n\
    #define t6 TEX6\n\
    #define t7 TEX7\n\
    out vec4 FragColor;\n\
    uniform int FrameDirection;\n\
    uniform int FrameCount;\n\
    uniform vec2 OutputSize;\n\
    uniform vec2 TextureSize;\n\
    uniform vec2 InputSize;\n\
    uniform sampler2D Texture;\n\
    in vec2 texCoord;\n\
    in vec4 t1;\n\
    in vec4 t2;\n\
    in vec4 t3;\n\
    in vec4 t4;\n\
    in vec4 t5;\n\
    in vec4 t6;\n\
    in vec4 t7;\n\
    const float coef = 2.0;\n\
    const vec3 rgbw = vec3(14.352, 28.176, 5.472);\n\
    const vec4 eq_threshold = vec4(15.0, 15.0, 15.0, 15.0);\n\
    vec4 delta = vec4(1.0 / XBR_SCALE, 1.0 / XBR_SCALE, 1.0 / XBR_SCALE, 1.0 / XBR_SCALE);\n\
    vec4 delta_l = vec4(0.5 / XBR_SCALE, 1.0 / XBR_SCALE, 0.5 / XBR_SCALE, 1.0 / XBR_SCALE);\n\
    vec4 delta_u = delta_l.yxwz;\n\
    const vec4 Ao = vec4(1.0, -1.0, -1.0, 1.0);\n\
    const vec4 Bo = vec4(1.0, 1.0, -1.0, -1.0);\n\
    const vec4 Co = vec4(1.5, 0.5, -0.5, 0.5);\n\
    const vec4 Ax = vec4(1.0, -1.0, -1.0, 1.0);\n\
    const vec4 Bx = vec4(0.5, 2.0, -0.5, -2.0);\n\
    const vec4 Cx = vec4(1.0, 1.0, -0.5, 0.0);\n\
    const vec4 Ay = vec4(1.0, -1.0, -1.0, 1.0);\n\
    const vec4 By = vec4(2.0, 0.5, -2.0, -0.5);\n\
    const vec4 Cy = vec4(2.0, 0.0, -1.0, 0.5);\n\
    const vec4 Ci = vec4(0.25, 0.25, 0.25, 0.25);\n\
    const vec3 Y = vec3(0.2126, 0.7152, 0.0722);\n\
    vec4 df(vec4 A, vec4 B)\n\
    {\n\
        return vec4(abs(A - B));\n\
    }\n\
    vec4 diff(vec4 A, vec4 B)\n\
    {\n\
        return vec4(notEqual(A, B));\n\
    }\n\
    vec4 eq(vec4 A, vec4 B)\n\
    {\n\
        return step(df(A, B), eq_threshold);\n\
    }\n\
    vec4 neq(vec4 A, vec4 B)\n\
    {\n\
        return vec4(1.0, 1.0, 1.0, 1.0) - eq(A, B);\n\
    }\n\
    vec4 wd(vec4 a, vec4 b, vec4 c, vec4 d, vec4 e, vec4 f, vec4 g, vec4 h)\n\
    {\n\
        return (df(a, b) + df(a, c) + df(d, e) + df(d, f) + 4.0 * df(g, h));\n\
    }\n\
    vec4 weighted_distance(vec4 a, vec4 b, vec4 c, vec4 d, vec4 e, vec4 f, vec4 g, vec4 h, vec4 i, vec4 j, vec4 k, vec4 l)\n\
    {\n\
        return (df(a, b) + df(a, c) + df(d, e) + df(d, f) + df(i, j) + df(k, l) + 2.0 * df(g, h));\n\
    }\n\
    float c_df(vec3 c1, vec3 c2)\n\
    {\n\
        vec3 df = abs(c1 - c2);\n\
        return df.r + df.g + df.b;\n\
    }\n\
    void main()\n\
    {\n\
        vec4 edri, edr, edr_l, edr_u, px;\n\
        vec4 irlv0, irlv1, irlv2l, irlv2u, block_3d;\n\
        vec4 fx, fx_l, fx_u;\n\
        vec2 fp = fract(texCoord * TextureSize);\n\
        vec3 A1 = texture(Texture, t1.xw).xyz;\n\
        vec3 B1 = texture(Texture, t1.yw).xyz;\n\
        vec3 C1 = texture(Texture, t1.zw).xyz;\n\
        vec3 A = texture(Texture, t2.xw).xyz;\n\
        vec3 B = texture(Texture, t2.yw).xyz;\n\
        vec3 C = texture(Texture, t2.zw).xyz;\n\
        vec3 D = texture(Texture, t3.xw).xyz;\n\
        vec3 E = texture(Texture, t3.yw).xyz;\n\
        vec3 F = texture(Texture, t3.zw).xyz;\n\
        vec3 G = texture(Texture, t4.xw).xyz;\n\
        vec3 H = texture(Texture, t4.yw).xyz;\n\
        vec3 I = texture(Texture, t4.zw).xyz;\n\
        vec3 G5 = texture(Texture, t5.xw).xyz;\n\
        vec3 H5 = texture(Texture, t5.yw).xyz;\n\
        vec3 I5 = texture(Texture, t5.zw).xyz;\n\
        vec3 A0 = texture(Texture, t6.xy).xyz;\n\
        vec3 D0 = texture(Texture, t6.xz).xyz;\n\
        vec3 G0 = texture(Texture, t6.xw).xyz;\n\
        vec3 C4 = texture(Texture, t7.xy).xyz;\n\
        vec3 F4 = texture(Texture, t7.xz).xyz;\n\
        vec3 I4 = texture(Texture, t7.xw).xyz;\n\
        vec4 b = vec4(dot(B, rgbw), dot(D, rgbw), dot(H, rgbw), dot(F, rgbw));\n\
        vec4 c = vec4(dot(C, rgbw), dot(A, rgbw), dot(G, rgbw), dot(I, rgbw));\n\
        vec4 d = b.yzwx;\n\
        vec4 e = vec4(dot(E, rgbw));\n\
        vec4 f = b.wxyz;\n\
        vec4 g = c.zwxy;\n\
        vec4 h = b.zwxy;\n\
        vec4 i = c.wxyz;\n\
        vec4 i4, i5, h5, f4;\n\
        i4 = vec4(dot(I4, rgbw), dot(C1, rgbw), dot(A0, rgbw), dot(G5, rgbw));\n\
        i5 = vec4(dot(I5, rgbw), dot(C4, rgbw), dot(A1, rgbw), dot(G0, rgbw));\n\
        h5 = vec4(dot(H5, rgbw), dot(F4, rgbw), dot(B1, rgbw), dot(D0, rgbw));\n\
        fx = (Ao * fp.y + Bo * fp.x);\n\
        fx_l = (Ax * fp.y + Bx * fp.x);\n\
        fx_u = (Ay * fp.y + By * fp.x);\n\
        irlv1 = irlv0 = diff(e, f) * diff(e, h);\n\
        irlv1 = (irlv0 * (neq(f, b) * neq(f, c) + neq(h, d) * neq(h, g) + eq(e, i) * (neq(f, f4) * neq(f, i4) + neq(h, h5) * neq(h, i5)) + eq(e, g) + eq(e, c)));\n\
        irlv2l = diff(e, g) * diff(d, g);\n\
        irlv2u = diff(e, c) * diff(b, c);\n\
        vec4 fx45i = clamp((fx + delta - Co - Ci) / (2.0 * delta), 0.0, 1.0);\n\
        vec4 fx45 = clamp((fx + delta - Co) / (2.0 * delta), 0.0, 1.0);\n\
        vec4 fx30 = clamp((fx_l + delta_l - Cx) / (2.0 * delta_l), 0.0, 1.0);\n\
        vec4 fx60 = clamp((fx_u + delta_u - Cy) / (2.0 * delta_u), 0.0, 1.0);\n\
        vec4 wd1, wd2;\n\
        wd1 = wd(e, c, g, i, h5, f4, h, f);\n\
        wd2 = wd(h, d, i5, f, i4, b, e, i);\n\
        edri = step(wd1, wd2) * irlv0;\n\
        edr = step(wd1 + vec4(0.1, 0.1, 0.1, 0.1), wd2) * step(vec4(0.5, 0.5, 0.5, 0.5), irlv1);\n\
        edr_l = step(lv2_cf * df(f, g), df(h, c)) * irlv2l * edr;\n\
        edr_u = step(lv2_cf * df(h, c), df(f, g)) * irlv2u * edr;\n\
        fx45 = edr * fx45;\n\
        fx30 = edr_l * fx30;\n\
        fx60 = edr_u * fx60;\n\
        fx45i = edri * fx45i;\n\
        px = step(df(e, f), df(e, h));\n\
        vec4 maximos = max(max(fx30, fx60), max(fx45, fx45i));\n\
        vec3 res1 = E;\n\
        res1 = mix(res1, mix(H, F, px.x), maximos.x);\n\
        res1 = mix(res1, mix(B, D, px.z), maximos.z);\n\
        vec3 res2 = E;\n\
        res2 = mix(res2, mix(F, B, px.y), maximos.y);\n\
        res2 = mix(res2, mix(D, H, px.w), maximos.w);\n\
        vec3 res = mix(res1, res2, step(c_df(E, res1), c_df(E, res2)));\n\
        FragColor = vec4(res, 1.0);\n\
    }\n";

const QUAD: [f32; 24] = [
    -1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 0.0, -1.0, -1.0,
    0.0, 1.0,
];

fn gl_pfd() -> PIXELFORMATDESCRIPTOR {
    let mut pfd: PIXELFORMATDESCRIPTOR = unsafe { std::mem::zeroed() };
    pfd.nSize = std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16;
    pfd.nVersion = 1;
    pfd.dwFlags = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER | PFD_SWAP_EXCHANGE;
    pfd.iPixelType = PFD_TYPE_RGBA;
    pfd.cColorBits = 32;
    pfd.cDepthBits = 0;
    pfd.iLayerType = 0;
    pfd
}

fn dll_dir() -> Option<String> {
    unsafe {
        let mut h = HMODULE::default();
        let addr = dll_dir as *const u8;
        const FLAG: u32 = windows::Win32::System::LibraryLoader::GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS;
        if GetModuleHandleExA(FLAG, PCSTR(addr), &mut h).is_ok() {
            let mut buf = [0u8; 1024];
            let n = GetModuleFileNameA(Some(h), &mut buf);
            if n > 0 {
                let path = String::from_utf8_lossy(&buf[..n as usize]);
                return std::path::Path::new(path.as_ref()).parent().map(|p| p.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn locate_shader(name: &str, shaderpath: &str) -> Option<String> {
    let p = std::path::Path::new(name);
    if p.is_file() {
        return Some(p.to_string_lossy().to_string());
    }
    let mut candidates: Vec<String> = Vec::new();
    if !shaderpath.trim().is_empty() {
        candidates.push(format!("{}\\{}", shaderpath.trim_end_matches('\\'), name));
    }
    if let Some(dir) = dll_dir() {
        candidates.push(format!("{}\\{}", dir, name));
    }
    for c in &candidates {
        if std::path::Path::new(c).is_file() {
            return Some(c.clone());
        }
    }
    None
}

fn split_shader_file(raw: &str) -> (String, String) {
    if let Some(pos) = raw.find("#version") {
        let rest = &raw[pos..];
        let end = rest.find('\n').unwrap_or(rest.len());
        let mut ver = rest[..end].to_string();
        if ver.starts_with("#version 130") || ver.starts_with("#version 140") {
            ver = "#version 150".to_string();
        }
        let body = rest[end..].to_string();
        (ver, body)
    } else {
        ("#version 150".to_string(), raw.to_string())
    }
}

fn read_shader(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

impl OglState {
    unsafe fn compile_shader(&self, kind: u32, source: &str) -> u32 {
        let cs = match std::ffi::CString::new(source) {
            Ok(c) => c,
            Err(_) => return 0,
        };
        let shader = (self.gl.create_shader)(kind);
        if shader == 0 {
            return 0;
        }
        let ptr = cs.as_ptr();
        (self.gl.shader_source)(shader, 1, &ptr, std::ptr::null());
        (self.gl.compile_shader)(shader);
        let mut ok = 0i32;
        (self.gl.get_shaderiv)(shader, GL_COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut len = 0i32;
            (self.gl.get_shaderiv)(shader, GL_INFO_LOG_LENGTH, &mut len);
            if len > 0 {
                let mut buf = vec![0u8; len as usize];
                let mut got = 0i32;
                (self.gl.get_shader_info_log)(shader, len, &mut got, buf.as_mut_ptr() as *mut i8);
                crate::dd_log!("opengl: shader compile error: {}", String::from_utf8_lossy(&buf));
            }
            (self.gl.delete_shader)(shader);
            return 0;
        }
        shader
    }

    unsafe fn build_program(&self, vs: &str, fs: &str) -> Option<BoundProgram> {
        let vert = self.compile_shader(GL_VERTEX_SHADER, vs);
        let frag = self.compile_shader(GL_FRAGMENT_SHADER, fs);
        if vert == 0 || frag == 0 {
            if vert != 0 {
                (self.gl.delete_shader)(vert);
            }
            if frag != 0 {
                (self.gl.delete_shader)(frag);
            }
            return None;
        }
        let prog = (self.gl.create_program)();
        if prog == 0 {
            (self.gl.delete_shader)(vert);
            (self.gl.delete_shader)(frag);
            return None;
        }
        (self.gl.attach_shader)(prog, vert);
        (self.gl.attach_shader)(prog, frag);
        (self.gl.link_program)(prog);
        let mut ok = 0i32;
        (self.gl.get_programiv)(prog, GL_LINK_STATUS, &mut ok);
        if ok == 0 {
            let mut len = 0i32;
            (self.gl.get_programiv)(prog, GL_INFO_LOG_LENGTH, &mut len);
            if len > 0 {
                let mut buf = vec![0u8; len as usize];
                let mut got = 0i32;
                (self.gl.get_program_info_log)(prog, len, &mut got, buf.as_mut_ptr() as *mut i8);
                crate::dd_log!("opengl: program link error: {}", String::from_utf8_lossy(&buf));
            }
            (self.gl.delete_program)(prog);
            (self.gl.delete_shader)(vert);
            (self.gl.delete_shader)(frag);
            return None;
        }
        (self.gl.detach_shader)(prog, vert);
        (self.gl.detach_shader)(prog, frag);
        (self.gl.delete_shader)(vert);
        (self.gl.delete_shader)(frag);

        let pos = unsafe { self.find_attr(prog, &[c"VertexCoord", c"a_position", c"position"]) };
        let uv =
            unsafe { self.find_attr(prog, &[c"TexCoord", c"a_tex_coord", c"v_tex_coord", c"tex_coord", c"texCoord"]) };
        let color = unsafe { self.find_attr(prog, &[c"Color", c"COLOR", c"a_color", c"v_color"]) };
        Some(BoundProgram { id: prog, pos, uv, color })
    }

    unsafe fn find_attr(&self, prog: u32, names: &[&CStr]) -> i32 {
        for n in names {
            let loc = (self.gl.get_attrib_location)(prog, n.as_ptr());
            if loc >= 0 {
                return loc;
            }
        }
        -1
    }

    unsafe fn build_builtin(&self, name: &str) -> Option<BoundProgram> {
        let (vs, fs) = match name {
            "passthrough" => (FIXED_VERT, NEAREST_FRAG),
            "bilinear" => (FIXED_VERT, BILINEAR_FRAG),
            "catmull" => (FIXED_VERT, CATMULL_ROM_FRAG),
            "lanczos" => (FIXED_VERT, LANCZOS2_FRAG),
            "xbr" => (XBR_LV2_VERT, XBR_LV2_FRAG),
            _ => (FIXED_VERT, CATMULL_ROM_FRAG),
        };
        self.build_program(vs, fs)
    }

    unsafe fn build_from_file(&self, path: &str) -> Option<BoundProgram> {
        let raw = read_shader(path)?;
        let (ver, body) = split_shader_file(&raw);
        let frag = format!("{}\n#define FRAGMENT\n{}", ver, body);
        if let Some(p) = self.build_program(FIXED_VERT, &frag) {
            return Some(p);
        }
        let vert = format!("{}\n#define VERTEX\n{}", ver, body);
        let frag2 = format!("{}\n#define FRAGMENT\n{}", ver, body);
        self.build_program(&vert, &frag2)
    }

    fn resolve_shader(shader: &str, shaderpath: &str, filter: i32) -> ShaderRef {
        let name = shader.trim();
        if name.is_empty() {
            return match filter {
                0 => ShaderRef::Builtin("passthrough"),
                1 => ShaderRef::Builtin("bilinear"),
                2 => ShaderRef::Builtin("catmull"),
                3 => ShaderRef::Builtin("lanczos"),
                _ => ShaderRef::Builtin("xbr"),
            };
        }
        let lower = name.to_ascii_lowercase();
        let looks_like_file = lower.contains(".glsl")
            || name.contains('\\')
            || name.contains('/')
            || std::path::Path::new(name).is_file();
        if looks_like_file {
            if let Some(p) = locate_shader(name, shaderpath) {
                return ShaderRef::File(p);
            }
            crate::dd_log!("opengl: shader '{}' not found; using default catmull-rom", name);
            return ShaderRef::Builtin("catmull");
        }
        match lower.as_str() {
            "nearest" | "nearest neighbor" | "point" => ShaderRef::Builtin("passthrough"),
            "bilinear" => ShaderRef::Builtin("bilinear"),
            "catmull" | "catmull-rom" | "catmullrom" | "bicubic" => ShaderRef::Builtin("catmull"),
            "lanczos" | "lanczos2" | "lanczos2-sharp" => ShaderRef::Builtin("lanczos"),
            "xbr" | "xbr-lv2" | "xbr-lv2-noblend" => ShaderRef::Builtin("xbr"),
            _ => {
                crate::dd_log!("opengl: unknown shader '{}'; using default catmull-rom", name);
                ShaderRef::Builtin("catmull")
            }
        }
    }

    fn sync_programs(&mut self, shader: &str, shaderpath: &str, shaderpath_pass1: &str, filter: i32) -> (bool, bool) {
        let main_src = Self::resolve_shader(shader, shaderpath, filter);
        let main_key = main_src.key();
        if main_key != self.prog_key {
            if let Some(old) = self.prog.take() {
                unsafe { (self.gl.delete_program)(old.id) };
            }
            let p = unsafe {
                match &main_src {
                    ShaderRef::Builtin(n) => self.build_builtin(n),
                    ShaderRef::File(path) => self.build_from_file(path),
                }
            };
            let p = match p {
                Some(p) => p,
                None => {
                    crate::dd_log!("opengl: failed to build program '{}'; using default catmull-rom", main_key);
                    unsafe { self.build_builtin("catmull") }.unwrap_or(BoundProgram {
                        id: 0,
                        pos: -1,
                        uv: -1,
                        color: -1,
                    })
                }
            };
            self.prog_key = main_key;
            self.prog = Some(p);
        }

        let pass_name = shaderpath_pass1.trim();
        let pass_key = if pass_name.is_empty() {
            String::new()
        } else {
            locate_shader(pass_name, shaderpath).unwrap_or_else(|| {
                crate::dd_log!("opengl: pass1 shader '{}' not found", pass_name);
                String::new()
            })
        };
        if pass_key != self.pass_key {
            if let Some(old) = self.pass_prog.take() {
                unsafe { (self.gl.delete_program)(old.id) };
            }
            self.pass_prog = if pass_key.is_empty() { None } else { unsafe { self.build_from_file(&pass_key) } };
            if self.pass_prog.is_none() && !pass_key.is_empty() {
                crate::dd_log!("opengl: pass1 shader '{}' failed; falling back to single pass", pass_key);
            }
            self.pass_key = pass_key;
        }

        (self.prog.as_ref().map(|p| p.id != 0).unwrap_or(false), self.pass_prog.is_some())
    }

    unsafe fn ensure_texture(&mut self, width: i32, height: i32, bpp: i32, rgb555: bool) {
        if width == self.surf_w && height == self.surf_h && bpp == self.surf_bpp {
            return;
        }
        let w = width.max(1);
        let h = height.max(1);
        if self.tex != 0 {
            (self.gl.delete_textures)(1, &self.tex);
        }
        (self.gl.gen_textures)(1, &mut self.tex);
        (self.gl.bind_texture)(GL_TEXTURE_2D, self.tex);
        (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST as i32);
        (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as i32);
        (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE as i32);
        (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE as i32);
        (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAX_LEVEL, 0);
        let (internal, format, gtype) = if bpp == 32 {
            (GL_RGBA8, GL_BGRA, GL_UNSIGNED_BYTE)
        } else if rgb555 {
            (GL_RGB5, GL_RGB, GL_UNSIGNED_SHORT_1_5_5_5_REV)
        } else {
            (GL_RGB565, GL_RGB, GL_UNSIGNED_SHORT_5_6_5)
        };
        (self.gl.tex_image_2d)(GL_TEXTURE_2D, 0, internal as i32, w, h, 0, format, gtype, std::ptr::null());
        self.surf_w = width;
        self.surf_h = height;
        self.surf_bpp = bpp;
        crate::dd_log!(
            "opengl: texture allocated for surface {}x{} bpp={} internal={:#x}",
            width,
            height,
            bpp,
            internal
        );
    }

    unsafe fn upload_surface(&mut self, buffers: &SurfaceBuffers, rgb555: bool) {
        let w = buffers.width.max(1);
        let h = buffers.height.max(1);
        if buffers.bpp == 8 {
            let n = (w as usize) * (h as usize);
            if self.stage.len() < n {
                self.stage.resize(n, 0);
            }
            scale::convert_scale(
                buffers.surface,
                buffers.pitch as usize,
                buffers.width,
                buffers.height,
                8,
                false,
                crate::state::active_palette_entries().as_ref(),
                0,
                &mut self.stage,
                buffers.width,
                buffers.height,
            );
            glPixelStorei(GL_UNPACK_ROW_LENGTH, w);
            (self.gl.tex_sub_image_2d)(
                GL_TEXTURE_2D,
                0,
                0,
                0,
                w,
                h,
                GL_BGRA,
                GL_UNSIGNED_BYTE,
                self.stage.as_ptr() as *const core::ffi::c_void,
            );
        } else if buffers.bpp == 16 {
            glPixelStorei(GL_UNPACK_ROW_LENGTH, (buffers.pitch / 2).max(1));
            let (format, gtype) =
                if rgb555 { (GL_RGB, GL_UNSIGNED_SHORT_1_5_5_5_REV) } else { (GL_RGB, GL_UNSIGNED_SHORT_5_6_5) };
            (self.gl.tex_sub_image_2d)(
                GL_TEXTURE_2D,
                0,
                0,
                0,
                w,
                h,
                format,
                gtype,
                buffers.surface as *const core::ffi::c_void,
            );
        } else {
            glPixelStorei(GL_UNPACK_ROW_LENGTH, (buffers.pitch / 4).max(1));
            (self.gl.tex_sub_image_2d)(
                GL_TEXTURE_2D,
                0,
                0,
                0,
                w,
                h,
                GL_BGRA,
                GL_UNSIGNED_BYTE,
                buffers.surface as *const core::ffi::c_void,
            );
        }
        glPixelStorei(GL_UNPACK_ROW_LENGTH, 0);
    }

    unsafe fn ensure_fbo(&mut self, w: i32, h: i32) -> bool {
        if self.fbo != 0 && self.fbo_w == w && self.fbo_h == h {
            return true;
        }
        let w = w.max(1);
        let h = h.max(1);
        if self.fbo_tex != 0 {
            (self.gl.delete_textures)(1, &self.fbo_tex);
            self.fbo_tex = 0;
        }
        if self.fbo != 0 {
            (self.gl.delete_framebuffers)(1, &self.fbo);
            self.fbo = 0;
        }
        (self.gl.gen_textures)(1, &mut self.fbo_tex);
        (self.gl.bind_texture)(GL_TEXTURE_2D, self.fbo_tex);
        (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR as i32);
        (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR as i32);
        (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE as i32);
        (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE as i32);
        (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAX_LEVEL, 0);
        (self.gl.tex_image_2d)(GL_TEXTURE_2D, 0, GL_RGBA8 as i32, w, h, 0, GL_RGBA, GL_UNSIGNED_BYTE, std::ptr::null());
        (self.gl.gen_framebuffers)(1, &mut self.fbo);
        (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
        (self.gl.framebuffer_texture_2d)(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, self.fbo_tex, 0);
        let status = (self.gl.check_framebuffer_status)(GL_FRAMEBUFFER);
        (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
        if status != GL_FRAMEBUFFER_COMPLETE {
            crate::dd_log!("opengl: FBO incomplete (status={:#x}); disabling two-pass", status);
            (self.gl.delete_textures)(1, &self.fbo_tex);
            self.fbo_tex = 0;
            (self.gl.delete_framebuffers)(1, &self.fbo);
            self.fbo = 0;
            return false;
        }
        self.fbo_w = w;
        self.fbo_h = h;
        true
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn set_uniforms(
        &self,
        prog: u32,
        in_w: i32,
        in_h: i32,
        tex_w: i32,
        tex_h: i32,
        out_w: i32,
        out_h: i32,
        frame_count: i32,
        unit1_tex: Option<u32>,
    ) {
        let gl = &self.gl;
        let set1i = |name: &CStr, v: i32| {
            let loc = (gl.get_uniform_location)(prog, name.as_ptr());
            if loc >= 0 {
                (gl.uniform_1i)(loc, v);
            }
        };
        let set2f = |name: &CStr, x: f32, y: f32| {
            let loc = (gl.get_uniform_location)(prog, name.as_ptr());
            if loc >= 0 {
                (gl.uniform_2f)(loc, x, y);
            }
        };
        for n in [c"Texture", c"texture", c"tex0", c"rubyTexture"] {
            set1i(n, 0);
        }
        for n in [c"InputSize", c"rubyInputSize", c"input_size", c"source_size"] {
            set2f(n, in_w as f32, in_h as f32);
        }
        for n in [c"TextureSize", c"rubyTextureSize", c"texture_size"] {
            set2f(n, tex_w as f32, tex_h as f32);
        }
        for n in [c"OutputSize", c"rubyOutputSize", c"output_size"] {
            set2f(n, out_w as f32, out_h as f32);
        }
        set2f(c"pixel_center", 0.5, 0.5);
        for n in [c"FrameCount", c"rubyFrameCount"] {
            set1i(n, frame_count);
        }
        for n in [c"FrameDirection", c"rubyFrameDirection"] {
            set1i(n, 1);
        }
        if let Some(loc) = opt_loc(gl, prog, c"MVPMatrix") {
            let id: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
            (gl.uniform_matrix4fv)(loc, 1, 0, id.as_ptr());
        }
        if let Some(tex) = unit1_tex {
            (gl.active_texture)(GL_TEXTURE0 + 1);
            (gl.bind_texture)(GL_TEXTURE_2D, tex);
            for n in [c"PassPrev2Texture", c"rubyPassPrev2Texture"] {
                set1i(n, 1);
            }
            for n in [c"PassPrev2TextureSize", c"rubyPassPrev2TextureSize"] {
                set2f(n, in_w as f32, in_h as f32);
            }
            (gl.active_texture)(GL_TEXTURE0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw(
        &self,
        bp: &BoundProgram,
        src_unit0: u32,
        unit1: Option<u32>,
        in_w: i32,
        in_h: i32,
        tex_w: i32,
        tex_h: i32,
        out_w: i32,
        out_h: i32,
        frame_count: i32,
    ) {
        let gl = &self.gl;
        if bp.id == 0 {
            return;
        }
        (gl.use_program)(bp.id);
        (gl.bind_vertex_array)(self.vao);
        (gl.bind_buffer)(GL_ARRAY_BUFFER, self.vbo);
        if bp.pos >= 0 {
            (gl.enable_vertex_attrib_array)(bp.pos as u32);
            (gl.vertex_attrib_pointer)(bp.pos as u32, 2, GL_FLOAT, 0, 16, std::ptr::null());
        }
        if bp.uv >= 0 {
            (gl.enable_vertex_attrib_array)(bp.uv as u32);
            (gl.vertex_attrib_pointer)(bp.uv as u32, 2, GL_FLOAT, 0, 16, 8usize as *const core::ffi::c_void);
        }
        if bp.color >= 0 {
            (gl.vertex_attrib4f)(bp.color as u32, 1.0, 1.0, 1.0, 1.0);
        }
        (gl.active_texture)(GL_TEXTURE0);
        (gl.bind_texture)(GL_TEXTURE_2D, src_unit0);
        self.set_uniforms(bp.id, in_w, in_h, tex_w, tex_h, out_w, out_h, frame_count, unit1);
        (gl.draw_arrays)(GL_TRIANGLES, 0, 6);
        if bp.pos >= 0 {
            (gl.disable_vertex_attrib_array)(bp.pos as u32);
        }
        if bp.uv >= 0 {
            (gl.disable_vertex_attrib_array)(bp.uv as u32);
        }
    }
}

fn opt_loc(gl: &Gl, prog: u32, name: &CStr) -> Option<i32> {
    let loc = unsafe { (gl.get_uniform_location)(prog, name.as_ptr()) };
    if loc >= 0 { Some(loc) } else { None }
}

pub(crate) struct OglState {
    hdc: HDC,
    ctx: HGLRC,
    gl: Gl,
    temp_dc: Option<HDC>,
    temp_bmp: Option<HBITMAP>,
    tex: u32,
    surf_w: i32,
    surf_h: i32,
    surf_bpp: i32,
    stage: Vec<u32>,
    vao: u32,
    vbo: u32,
    prog: Option<BoundProgram>,
    prog_key: String,
    pass_prog: Option<BoundProgram>,
    pass_key: String,
    fbo: u32,
    fbo_tex: u32,
    fbo_w: i32,
    fbo_h: i32,
    frame_count: i32,
}

impl OglState {
    pub(crate) fn new(hdc: HDC, width: i32, height: i32) -> Option<OglState> {
        unsafe {
            if GetPixelFormat(hdc) == 0 {
                let pfd = gl_pfd();
                let idx = ChoosePixelFormat(hdc, &pfd);
                if idx == 0 {
                    crate::dd_log!("opengl: ChoosePixelFormat failed, no GL pixel format available");
                    return None;
                }
                if SetPixelFormat(hdc, idx, &pfd).is_err() {
                    crate::dd_log!("opengl: SetPixelFormat failed, no GL pixel format available");
                    return None;
                }
            }

            let mem = CreateCompatibleDC(Some(hdc));
            if mem.is_invalid() {
                return None;
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: 64,
                    biHeight: -64,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [RGBQUAD::default(); 1],
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = match CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) => b,
                Err(_) => {
                    let _ = DeleteDC(mem);
                    return None;
                }
            };
            let _ = SelectObject(mem, HGDIOBJ(bmp.0));
            let pfd = gl_pfd();
            let mem_idx = ChoosePixelFormat(mem, &pfd);
            if mem_idx == 0 {
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                let _ = DeleteDC(mem);
                return None;
            }
            let _ = SetPixelFormat(mem, mem_idx, &pfd);
            let temp_dc = Some(mem);
            let temp_bmp = Some(bmp);

            let legacy = match wglCreateContext(mem) {
                Ok(c) => c,
                Err(_) => {
                    let _ = DeleteObject(HGDIOBJ(bmp.0));
                    let _ = DeleteDC(mem);
                    return None;
                }
            };
            if wglMakeCurrent(mem, legacy).is_err() {
                let _ = wglDeleteContext(legacy);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                let _ = DeleteDC(mem);
                return None;
            }

            let create_ctx_attr: Option<CreateContextAttribsFn> = load_proc(b"wglCreateContextAttribsARB\0");
            let choose_pf: Option<ChoosePixelFormatArbFn> = load_proc(b"wglChoosePixelFormatARB\0");
            let _swap_interval: Option<SwapIntervalFn> = load_proc(b"wglSwapIntervalEXT\0");

            if create_ctx_attr.is_none() {
                crate::dd_log!("opengl: WGL_ARB_create_context not available, 3.2 core required");
                let _ = wglMakeCurrent(mem, HGLRC(std::ptr::null_mut()));
                let _ = wglDeleteContext(legacy);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                let _ = DeleteDC(mem);
                return None;
            }

            if let Some(choose) = choose_pf {
                let attrs: [i32; 13] = [
                    WGL_DRAW_TO_WINDOW_ARB,
                    1,
                    WGL_SUPPORT_OPENGL_ARB,
                    1,
                    WGL_DOUBLE_BUFFER_ARB,
                    1,
                    WGL_PIXEL_TYPE_ARB,
                    WGL_TYPE_RGBA_ARB,
                    WGL_COLOR_BITS_ARB,
                    32,
                    0,
                    0,
                    0,
                ];
                let mut fmt = 0i32;
                let mut num = 0u32;
                if choose(hdc, attrs.as_ptr(), std::ptr::null(), 1, &mut fmt, &mut num) != 0 && fmt != 0 {
                    crate::dd_log!("opengl: using WGL_ARB pixel format {}", fmt);
                    let _ = SetPixelFormat(hdc, fmt, std::ptr::null());
                }
            }

            let attribs: [i32; 10] = [
                WGL_CONTEXT_MAJOR_VERSION_ARB,
                3,
                WGL_CONTEXT_MINOR_VERSION_ARB,
                2,
                WGL_CONTEXT_FLAGS_ARB,
                WGL_CONTEXT_FORWARD_COMPATIBLE_BIT_ARB,
                WGL_CONTEXT_PROFILE_MASK_ARB,
                WGL_CONTEXT_CORE_PROFILE_BIT_ARB,
                0,
                0,
            ];
            let core = (create_ctx_attr.unwrap())(hdc, HGLRC(std::ptr::null_mut()), attribs.as_ptr());
            if core.is_invalid() {
                crate::dd_log!("opengl: wglCreateContextAttribsARB failed to create a 3.2 core context");
                let _ = wglMakeCurrent(mem, HGLRC(std::ptr::null_mut()));
                let _ = wglDeleteContext(legacy);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                let _ = DeleteDC(mem);
                return None;
            }
            let _ = wglMakeCurrent(mem, HGLRC(std::ptr::null_mut()));

            if wglMakeCurrent(hdc, core).is_err() {
                crate::dd_log!("opengl: wglMakeCurrent failed for core context");
                let _ = wglDeleteContext(core);
                let _ = wglDeleteContext(legacy);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                let _ = DeleteDC(mem);
                return None;
            }

            let gl = match Gl::load() {
                Some(gl) => gl,
                None => {
                    let _ = wglMakeCurrent(hdc, HGLRC(std::ptr::null_mut()));
                    let _ = wglDeleteContext(core);
                    let _ = wglDeleteContext(legacy);
                    let _ = DeleteObject(HGDIOBJ(bmp.0));
                    let _ = DeleteDC(mem);
                    return None;
                }
            };

            let _ = wglDeleteContext(legacy);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(mem);

            glDisable(GL_DEPTH_TEST);

            let mut vao = 0u32;
            (gl.gen_vertex_arrays)(1, &mut vao);
            let mut vbo = 0u32;
            (gl.gen_buffers)(1, &mut vbo);
            (gl.bind_vertex_array)(vao);
            (gl.bind_buffer)(GL_ARRAY_BUFFER, vbo);
            (gl.buffer_data)(
                GL_ARRAY_BUFFER,
                std::mem::size_of_val(&QUAD) as isize,
                QUAD.as_ptr() as *const core::ffi::c_void,
                GL_STATIC_DRAW,
            );

            let version = glGetString(GL_VERSION);
            let version = if version.is_null() {
                String::from("<unknown>")
            } else {
                std::ffi::CStr::from_ptr(version as *const i8).to_string_lossy().into_owned()
            };
            crate::dd_log!("opengl: core context initialized, GL_VERSION={}", version);

            let rgb555 = crate::state::RGB555.load(Ordering::Relaxed);
            let mut st = OglState {
                hdc,
                ctx: core,
                gl,
                temp_dc,
                temp_bmp,
                tex: 0,
                surf_w: -1,
                surf_h: -1,
                surf_bpp: -1,
                stage: Vec::new(),
                vao,
                vbo,
                prog: None,
                prog_key: String::new(),
                pass_prog: None,
                pass_key: String::new(),
                fbo: 0,
                fbo_tex: 0,
                fbo_w: 0,
                fbo_h: 0,
                frame_count: 0,
            };
            st.ensure_texture(width.max(1), height.max(1), 32, rgb555);
            Some(st)
        }
    }

    pub(crate) fn present(&mut self, buffers: &SurfaceBuffers, upload: bool) {
        let (
            hwnd,
            gl_finish,
            gl_fence_sync,
            rgb555,
            shader,
            shaderpath,
            shaderpath_pass1,
            filter,
            render_w,
            render_h,
            viewport,
        ) = {
            let st = state().lock().unwrap();
            (
                st.hwnd,
                st.gl_finish,
                st.gl_fence_sync,
                crate::state::RGB555.load(Ordering::Relaxed),
                st.shader.clone(),
                st.shaderpath.clone(),
                st.shaderpath_pass1.clone(),
                st.filter,
                st.render.width,
                st.render.height,
                st.render.viewport,
            )
        };

        unsafe {
            if wglMakeCurrent(self.hdc, self.ctx).is_err() {
                return;
            }

            let upload_bpp = if buffers.bpp == 8 { 32 } else { buffers.bpp };
            self.ensure_texture(buffers.width, buffers.height, upload_bpp, rgb555);

            if upload {
                let _guard = buffers.lock.lock();
                self.upload_surface(buffers, rgb555);
                if gl_fence_sync {
                    let sync = (self.gl.fence_sync)(GL_SYNC_GPU_COMMANDS_COMPLETE, GL_SYNC_FLUSH_COMMANDS_BIT);
                    if !sync.is_null() {
                        (self.gl.client_wait_sync)(sync, GL_SYNC_FLUSH_COMMANDS_BIT, 0);
                        (self.gl.delete_sync)(sync);
                    }
                }
                drop(_guard);
            }

            let (main_ok, pass_needed) = self.sync_programs(&shader, &shaderpath, &shaderpath_pass1, filter);
            if !main_ok {
                crate::render::composite_child_windows(hwnd, buffers.hdc);
                return;
            }

            let w = buffers.width.max(1);
            let h = buffers.height.max(1);
            let rw = render_w.max(1);
            let rh = render_h.max(1);

            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if !hwnd.is_invalid() {
                GetClientRect(hwnd, &mut rc);
            }
            let cw = rc.right - rc.left;
            let ch = rc.bottom - rc.top;

            let (vl, vt, vr, vb, render_h2) = if viewport.right > viewport.left && viewport.bottom > viewport.top {
                (viewport.left, viewport.top, viewport.right, viewport.bottom, render_h)
            } else if cw > 0 && ch > 0 {
                (0, 0, cw, ch, ch)
            } else {
                (0, 0, w, h, h)
            };

            let fbo_tex_out = if pass_needed && self.ensure_fbo(rw, rh) {
                if let Some(p) = &self.pass_prog {
                    (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
                    glViewport(0, 0, self.fbo_w, self.fbo_h);
                    glClearColor(0.0, 0.0, 0.0, 1.0);
                    glClear(GL_COLOR_BUFFER_BIT);
                    self.draw(p, self.tex, None, w, h, w, h, self.fbo_w, self.fbo_h, self.frame_count);
                    (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
                }
                Some(self.fbo_tex)
            } else {
                None
            };

            if vr > vl && vb > vt {
                let gl_y = render_h2 - vt - (vb - vt);
                glViewport(vl, gl_y, vr - vl, vb - vt);
            } else {
                glViewport(0, 0, w, h);
            }
            glClearColor(0.0, 0.0, 0.0, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);

            if let Some(main) = &self.prog {
                let (src, unit1) = match fbo_tex_out {
                    Some(ft) => (ft, Some(self.tex)),
                    None => (self.tex, None),
                };
                let (out_w, out_h) = if vr > vl && vb > vt { (vr - vl, vb - vt) } else { (w, h) };
                let tex_w = match fbo_tex_out {
                    Some(_) => self.fbo_w,
                    None => w,
                };
                let tex_h = match fbo_tex_out {
                    Some(_) => self.fbo_h,
                    None => h,
                };
                self.draw(main, src, unit1, w, h, tex_w, tex_h, out_w, out_h, self.frame_count);
            }

            if gl_finish {
                glFinish();
            }

            crate::render::composite_child_windows(hwnd, buffers.hdc);

            self.frame_count = self.frame_count.wrapping_add(1);
        }
    }

    pub(crate) fn release(self) {
        unsafe {
            if let Some(p) = self.pass_prog {
                (self.gl.delete_program)(p.id);
            }
            if let Some(p) = self.prog {
                (self.gl.delete_program)(p.id);
            }
            if self.fbo_tex != 0 {
                (self.gl.delete_textures)(1, &self.fbo_tex);
            }
            if self.fbo != 0 {
                (self.gl.delete_framebuffers)(1, &self.fbo);
            }
            if self.vbo != 0 {
                (self.gl.delete_buffers)(1, &self.vbo);
            }
            if self.vao != 0 {
                (self.gl.delete_vertex_arrays)(1, &self.vao);
            }
            if self.tex != 0 {
                (self.gl.delete_textures)(1, &self.tex);
            }
            let _ = wglMakeCurrent(self.hdc, HGLRC(std::ptr::null_mut()));
            let _ = wglDeleteContext(self.ctx);
            if let Some(dc) = self.temp_dc {
                let _ = DeleteDC(dc);
            }
            if let Some(bmp) = self.temp_bmp {
                let _ = DeleteObject(HGDIOBJ(bmp.0));
            }
        }
    }
}
