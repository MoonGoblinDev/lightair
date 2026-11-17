// Shared by every data/fx/*.effect (prepended by the lightair_fx filter).
uniform float4x4 ViewProj;
uniform texture2d image;   // the input (or the previous pass)
uniform texture2d source;  // the input, in multi-pass effects
uniform float2 size;       // px
uniform float2 texel;      // 1 / size
uniform float time;        // seconds

sampler_state linearClamp {
	Filter    = Linear;
	AddressU  = Clamp;
	AddressV  = Clamp;
};

sampler_state pointClamp {
	Filter    = Point;
	AddressU  = Clamp;
	AddressV  = Clamp;
};

struct VertData {
	float4 pos : POSITION;
	float2 uv  : TEXCOORD0;
};

VertData VSDefault(VertData v_in)
{
	VertData vert_out;
	vert_out.pos = mul(float4(v_in.pos.xyz, 1.0), ViewProj);
	vert_out.uv  = v_in.uv;
	return vert_out;
}

float luma(float3 c)
{
	return dot(c, float3(0.2126, 0.7152, 0.0722));
}

float hash12(float2 p)
{
	float3 p3 = frac(float3(p.xyx) * 0.1031);
	p3 += dot(p3, p3.yzx + 33.33);
	return frac((p3.x + p3.y) * p3.z);
}

// Straight colour of a premultiplied sample.
float3 unpremul(float4 c)
{
	return c.a > 0.0001 ? c.rgb / c.a : c.rgb;
}

// Uv in a square space centred on the frame (x scaled by the aspect), and back.
float2 to_square(float2 uv)
{
	return float2((uv.x - 0.5) * size.x / size.y, uv.y - 0.5);
}

float2 from_square(float2 p)
{
	return float2(p.x * size.y / size.x + 0.5, p.y + 0.5);
}

// 9-tap gaussian along dir (px per tap step).
float4 gauss9(float2 uv, float2 dir)
{
	float2 o = dir * texel;
	float4 s = image.Sample(linearClamp, uv) * 0.2270270;
	s += (image.Sample(linearClamp, uv + o * 1.3846154) + image.Sample(linearClamp, uv - o * 1.3846154)) * 0.3162162;
	s += (image.Sample(linearClamp, uv + o * 3.2307692) + image.Sample(linearClamp, uv - o * 3.2307692)) * 0.0702703;
	return s;
}
