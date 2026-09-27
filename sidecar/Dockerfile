FROM python:3.12-slim
RUN pip install --no-cache-dir --index-url https://download.pytorch.org/whl/cpu torch==2.14.0 \
 && pip install --no-cache-dir laya==0.3.20
# No root at runtime: the server only needs to read its code and write the model cache.
RUN useradd --system --uid 10001 --no-create-home laya && mkdir /models && chown laya /models
WORKDIR /app
COPY laya_server.py .
ENV LAYA_HOST=0.0.0.0 LAYA_PORT=8771 HF_HOME=/models
VOLUME /models
USER laya
EXPOSE 8771
CMD ["python", "laya_server.py"]
